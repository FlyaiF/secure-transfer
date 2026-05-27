use std::io::Read;
use std::time::{Duration, Instant};

use anyhow::Result;
use image::GrayImage;

use transfer_common::fountain::Decoder;
use transfer_common::protocol;

// ─── Per-frame decode state ─────────────────────────────────────────────────

pub struct DecodeState {
    decoder: Option<Decoder>,
    encrypted_size: Option<u32>,
    unique_count: u32,
}

impl DecodeState {
    pub fn new() -> Self {
        Self {
            decoder: None,
            encrypted_size: None,
            unique_count: 0,
        }
    }

    pub fn decoded_count(&self) -> usize {
        self.decoder.as_ref().map_or(0, |d| d.decoded_count())
    }

    pub fn total_blocks(&self) -> usize {
        self.decoder.as_ref().map_or(0, |d| d.total_blocks())
    }

    /// Feed a captured frame. Returns the reassembled encrypted payload once
    /// enough fountain blocks have been collected.
    pub fn process_frame(&mut self, gray: &GrayImage, elapsed: Duration) -> Option<Vec<u8>> {
        let data = decode_qr_from_image(gray)?;
        let frame = protocol::decode_frame(&data)?;

        if self.decoder.is_none() {
            eprintln!(
                "First frame received! {} source blocks, {} bytes encrypted, block size {}",
                frame.block.total_blocks, frame.encrypted_size, frame.block_size
            );
            self.decoder = Some(Decoder::new(
                frame.block.total_blocks,
                frame.block_size as usize,
            ));
            self.encrypted_size = Some(frame.encrypted_size);
        }

        // Reject frames from a different transfer (e.g. sender restarted with
        // different file or block size).
        let expected_enc_size = self.encrypted_size.unwrap();
        let dec_ref = self.decoder.as_ref().unwrap();
        let expected_blocks = dec_ref.total_blocks() as u16;
        let expected_block_size = dec_ref.block_size() as u16;
        if frame.encrypted_size != expected_enc_size
            || frame.block.total_blocks != expected_blocks
            || frame.block_size != expected_block_size
        {
            return None;
        }

        let dec = self.decoder.as_mut().unwrap();
        if dec.add_block(&frame.block) {
            self.unique_count += 1;
        }

        eprint!(
            "\rReceived: {} unique | Decoded: {}/{} ({:.0}%) | {:.1}s  ",
            self.unique_count,
            dec.decoded_count(),
            dec.total_blocks(),
            dec.decoded_count() as f64 / dec.total_blocks() as f64 * 100.0,
            elapsed.as_secs_f64(),
        );

        if dec.is_complete() {
            eprintln!("\nAll blocks received!");
            let enc_size = self.encrypted_size.unwrap() as usize;
            return dec.reassemble(enc_size);
        }
        None
    }
}

impl Default for DecodeState {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Source-agnostic decode driver ──────────────────────────────────────────

/// Pull frames from `producer` until the encrypted payload is fully decoded.
///
/// The producer returns `None` to signal end-of-stream. If the stream ends
/// before decoding finishes, an error is returned with the partial progress.
pub fn run_decode_loop<F>(mut producer: F) -> Result<Vec<u8>>
where
    F: FnMut() -> Option<GrayImage>,
{
    let mut state = DecodeState::new();
    let start = Instant::now();

    while let Some(gray) = producer() {
        if let Some(encrypted) = state.process_frame(&gray, start.elapsed()) {
            return Ok(encrypted);
        }
    }

    anyhow::bail!(
        "stream ended before transfer complete: {}/{} blocks decoded",
        state.decoded_count(),
        state.total_blocks()
    );
}

/// Read fixed-size BGRA frames from `reader` and drive the decode loop.
///
/// `every` skips frames cheaply *before* the QR decode step: when `every > 1`,
/// only every Nth frame is converted and decoded. The reader is still drained
/// so the pipe stays in sync with the producer.
pub fn run_pipe_decode<R: Read>(
    mut reader: R,
    width: u32,
    height: u32,
    every: u32,
) -> Result<Vec<u8>> {
    if width == 0 || height == 0 {
        anyhow::bail!("--width and --height must be greater than 0");
    }
    let frame_size = (width as usize) * (height as usize) * 4;
    let mut buf = vec![0u8; frame_size];
    let mut frame_num: u32 = 0;

    run_decode_loop(|| loop {
        if reader.read_exact(&mut buf).is_err() {
            eprintln!("\nEnd of input stream");
            return None;
        }
        frame_num = frame_num.wrapping_add(1);
        if every > 1 && !frame_num.is_multiple_of(every) {
            // Drop this frame from the QR-decode path; the bytes were already
            // consumed from the reader above, so the pipe stays aligned.
            continue;
        }
        return Some(bgra_to_gray(&buf, width, height));
    })
}

// ─── Image helpers ──────────────────────────────────────────────────────────

pub fn decode_qr_from_image(image: &GrayImage) -> Option<Vec<u8>> {
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(
        image.width() as usize,
        image.height() as usize,
        |x, y| image.get_pixel(x as u32, y as u32).0[0],
    );
    let grids = prepared.detect_grids();
    for grid in grids {
        let mut data = Vec::new();
        if grid.decode_to(&mut data).is_ok() {
            return Some(data);
        }
    }
    None
}

pub fn bgra_to_gray(bgra: &[u8], w: u32, h: u32) -> GrayImage {
    GrayImage::from_fn(w, h, |x, y| {
        let i = ((y * w + x) * 4) as usize;
        let b = bgra[i] as f32;
        let g = bgra[i + 1] as f32;
        let r = bgra[i + 2] as f32;
        let luma = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
        image::Luma([luma])
    })
}

pub fn rgba_to_gray(rgba: &[u8], w: u32, h: u32) -> GrayImage {
    GrayImage::from_fn(w, h, |x, y| {
        let i = ((y * w + x) * 4) as usize;
        let r = rgba[i] as f32;
        let g = rgba[i + 1] as f32;
        let b = rgba[i + 2] as f32;
        let luma = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
        image::Luma([luma])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    use qrcode::{EcLevel, QrCode};
    use transfer_common::crypto;
    use transfer_common::fountain::{encode_block, split_into_blocks, DEFAULT_BLOCK_SIZE};
    use transfer_common::protocol::encode_frame;

    // ── Pure conversion correctness ────────────────────────────────────────

    #[test]
    fn bgra_to_gray_luma_weights() {
        // Pure red pixel: B=0, G=0, R=255 → luma 0.299*255 ≈ 76
        let pixels = [0u8, 0, 255, 255];
        let g = bgra_to_gray(&pixels, 1, 1);
        assert_eq!(g.get_pixel(0, 0).0[0], 76);

        // Pure green: B=0 G=255 R=0 → luma 0.587*255 ≈ 149
        let pixels = [0u8, 255, 0, 255];
        let g = bgra_to_gray(&pixels, 1, 1);
        assert_eq!(g.get_pixel(0, 0).0[0], 149);

        // Pure blue: B=255 G=0 R=0 → luma 0.114*255 ≈ 29
        let pixels = [255u8, 0, 0, 255];
        let g = bgra_to_gray(&pixels, 1, 1);
        assert_eq!(g.get_pixel(0, 0).0[0], 29);
    }

    #[test]
    fn rgba_to_gray_luma_weights() {
        // Pure red: R=255 G=0 B=0 → 76
        assert_eq!(
            rgba_to_gray(&[255, 0, 0, 255], 1, 1).get_pixel(0, 0).0[0],
            76
        );
        // Pure green: 149
        assert_eq!(
            rgba_to_gray(&[0, 255, 0, 255], 1, 1).get_pixel(0, 0).0[0],
            149
        );
        // Pure blue: 29
        assert_eq!(
            rgba_to_gray(&[0, 0, 255, 255], 1, 1).get_pixel(0, 0).0[0],
            29
        );
    }

    #[test]
    fn rgba_and_bgra_agree_on_grayscale_values() {
        // For neutral colors (R==G==B) both functions must produce the same
        // luma — this guarantees swapping conversions doesn't change the QR
        // decode path for white-on-black QR codes.
        for v in [0u8, 64, 128, 200, 255] {
            let bgra = [v, v, v, 255];
            let rgba = [v, v, v, 255];
            let lb = bgra_to_gray(&bgra, 1, 1).get_pixel(0, 0).0[0];
            let lr = rgba_to_gray(&rgba, 1, 1).get_pixel(0, 0).0[0];
            assert_eq!(lb, lr, "mismatch at v={v}");
        }
    }

    // ── QR round-trip through both conversion paths ────────────────────────

    /// Render a QR code into an RGBA framebuffer at the given scale, with a
    /// quiet zone, so we can exercise the full capture-style pipeline.
    fn render_qr_rgba(code: &QrCode, scale: u32) -> (Vec<u8>, u32, u32) {
        let width = code.width() as u32;
        let quiet = 4 * scale;
        let img_size = width * scale + 2 * quiet;
        let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_pixel(img_size, img_size, Rgba([255, 255, 255, 255]));

        for y in 0..width {
            for x in 0..width {
                if code[(x as usize, y as usize)] == qrcode::Color::Dark {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = x * scale + quiet + dx;
                            let py = y * scale + quiet + dy;
                            img.put_pixel(px, py, Rgba([0, 0, 0, 255]));
                        }
                    }
                }
            }
        }
        let (w, h) = (img.width(), img.height());
        (img.into_raw(), w, h)
    }

    fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
        rgba.chunks_exact(4)
            .flat_map(|c| [c[2], c[1], c[0], c[3]])
            .collect()
    }

    #[test]
    fn qr_round_trip_via_rgba_conversion() {
        let payload = b"hello world";
        let code = QrCode::with_error_correction_level(payload, EcLevel::M).unwrap();
        let (rgba, w, h) = render_qr_rgba(&code, 4);
        let gray = rgba_to_gray(&rgba, w, h);
        let decoded = decode_qr_from_image(&gray).expect("QR should decode");
        assert_eq!(decoded, payload);
    }

    #[test]
    fn qr_round_trip_via_bgra_conversion() {
        let payload = b"hello world";
        let code = QrCode::with_error_correction_level(payload, EcLevel::M).unwrap();
        let (rgba, w, h) = render_qr_rgba(&code, 4);
        let bgra = rgba_to_bgra(&rgba);
        let gray = bgra_to_gray(&bgra, w, h);
        let decoded = decode_qr_from_image(&gray).expect("QR should decode");
        assert_eq!(decoded, payload);
    }

    // ── End-to-end loop test ───────────────────────────────────────────────

    /// Drive `run_decode_loop` with a synthetic frame producer that emits QR
    /// codes for each fountain block. This is the same exercise the real
    /// receiver runs, minus the screen-capture layer.
    #[test]
    fn run_decode_loop_completes_a_full_transfer() {
        let plaintext = b"the quick brown fox jumps over the lazy dog, several times over";
        let (privkey, pubkey) = crypto::keygen();
        let encrypted = crypto::encrypt(plaintext, &pubkey).unwrap();
        let encrypted_size = encrypted.len() as u32;

        let source_blocks = split_into_blocks(&encrypted, DEFAULT_BLOCK_SIZE).unwrap();
        let k = source_blocks.len() as u16;

        let mut seed: u32 = 0;
        let producer = || -> Option<GrayImage> {
            // Generate an unbounded sequence of QR frames — `run_decode_loop`
            // will stop pulling once it has enough.
            let block = encode_block(&source_blocks, seed, DEFAULT_BLOCK_SIZE);
            let frame = encode_frame(&block, encrypted_size).unwrap();
            seed += 1;
            let code = QrCode::with_error_correction_level(&frame, EcLevel::M).unwrap();
            let (rgba, w, h) = render_qr_rgba(&code, 4);
            Some(rgba_to_gray(&rgba, w, h))
        };

        let reassembled = run_decode_loop(producer).expect("decode loop");
        let decrypted = crypto::decrypt(&reassembled, &privkey).unwrap();
        assert_eq!(decrypted, plaintext);
        assert!(seed >= k as u32, "should have consumed at least k frames");
    }

    /// If the producer runs out before enough unique blocks land, the loop
    /// must report partial progress rather than hang.
    #[test]
    fn run_decode_loop_errors_when_stream_ends_early() {
        let plaintext = vec![0xABu8; DEFAULT_BLOCK_SIZE * 4];
        let (_privkey, pubkey) = crypto::keygen();
        let encrypted = crypto::encrypt(&plaintext, &pubkey).unwrap();
        let encrypted_size = encrypted.len() as u32;
        let source_blocks = split_into_blocks(&encrypted, DEFAULT_BLOCK_SIZE).unwrap();

        // Yield only one frame, then end-of-stream.
        let mut sent = 0u32;
        let producer = || -> Option<GrayImage> {
            if sent > 0 {
                return None;
            }
            sent += 1;
            let block = encode_block(&source_blocks, 0, DEFAULT_BLOCK_SIZE);
            let frame = encode_frame(&block, encrypted_size).unwrap();
            let code = QrCode::with_error_correction_level(&frame, EcLevel::M).unwrap();
            let (rgba, w, h) = render_qr_rgba(&code, 4);
            Some(rgba_to_gray(&rgba, w, h))
        };

        let err = run_decode_loop(producer).expect_err("should fail");
        let msg = err.to_string();
        assert!(msg.contains("stream ended"), "got: {msg}");
    }

    /// Pipe-mode wrapper: synthesize a stream of BGRA frames in memory and
    /// feed it to `run_pipe_decode`, mirroring what stdin would deliver.
    #[test]
    fn run_pipe_decode_completes_via_bgra_stream() {
        let plaintext = b"pipe mode integration sanity";
        let (privkey, pubkey) = crypto::keygen();
        let encrypted = crypto::encrypt(plaintext, &pubkey).unwrap();
        let encrypted_size = encrypted.len() as u32;
        let source_blocks = split_into_blocks(&encrypted, DEFAULT_BLOCK_SIZE).unwrap();

        let (mut canvas_w, mut canvas_h) = (0u32, 0u32);
        let mut stream: Vec<u8> = Vec::new();

        // Generate enough frames to allow decoding with margin for fountain
        // overhead. All frames must share dimensions because pipe mode reads
        // fixed-size chunks.
        for seed in 0..(source_blocks.len() as u32 * 6 + 30) {
            let block = encode_block(&source_blocks, seed, DEFAULT_BLOCK_SIZE);
            let frame_bytes = encode_frame(&block, encrypted_size).unwrap();
            let code = QrCode::with_error_correction_level(&frame_bytes, EcLevel::M).unwrap();
            let (rgba, w, h) = render_qr_rgba(&code, 4);
            if canvas_w == 0 {
                canvas_w = w;
                canvas_h = h;
            }
            assert_eq!((w, h), (canvas_w, canvas_h), "QR canvas size must match");
            stream.extend_from_slice(&rgba_to_bgra(&rgba));
        }

        let reassembled = run_pipe_decode(stream.as_slice(), canvas_w, canvas_h, 1).unwrap();
        let decrypted = crypto::decrypt(&reassembled, &privkey).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn run_pipe_decode_rejects_zero_dimensions() {
        let stream: &[u8] = &[];
        let err = run_pipe_decode(stream, 0, 10, 1).unwrap_err();
        assert!(err.to_string().contains("greater than 0"));
    }

    /// `--every N` should drop every non-Nth frame before QR decode runs.
    ///
    /// We exercise this with a multi-block payload (so one QR can't finish
    /// the transfer) and a 2-frame stream `[garbage, valid_qr]`:
    /// - `every=2` skips frame 1 and processes frame 2 → the QR is decoded
    ///   and the fountain header is parsed, so the loop reports `X/K` where
    ///   K = total source blocks > 0.
    /// - `every=1` (control) processes both frames; frame 1 is garbage, but
    ///   frame 2 still lands the same header.
    ///
    /// The complementary "skip the valid frame" case is covered separately
    /// below to rule out the assertion succeeding by accident.
    #[test]
    fn run_pipe_decode_every_skips_intermediate_frames() {
        let plaintext = vec![0xAAu8; DEFAULT_BLOCK_SIZE * 4]; // > 1 block
        let (_privkey, pubkey) = crypto::keygen();
        let encrypted = crypto::encrypt(&plaintext, &pubkey).unwrap();
        let encrypted_size = encrypted.len() as u32;
        let source_blocks = split_into_blocks(&encrypted, DEFAULT_BLOCK_SIZE).unwrap();
        let k = source_blocks.len();
        assert!(k > 1, "test payload must span multiple blocks");

        // Two frames sharing dimensions (pipe mode reads fixed-size chunks).
        let block = encode_block(&source_blocks, 0, DEFAULT_BLOCK_SIZE);
        let frame_bytes = encode_frame(&block, encrypted_size).unwrap();
        let code = QrCode::with_error_correction_level(&frame_bytes, EcLevel::M).unwrap();
        let (rgba_qr, w, h) = render_qr_rgba(&code, 4);
        let bgra_qr = rgba_to_bgra(&rgba_qr);
        let bgra_garbage = vec![255u8; (w * h * 4) as usize]; // all-white, no QR

        let mut stream: Vec<u8> = Vec::new();
        stream.extend_from_slice(&bgra_garbage);
        stream.extend_from_slice(&bgra_qr);

        // With every=2, the QR on frame 2 is decoded and the header parsed,
        // so the error message carries the real source-block count (K).
        let err = run_pipe_decode(stream.as_slice(), w, h, 2).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&format!("/{} blocks", k)),
            "every=2 should have parsed the QR header (denominator {}); got: {msg}",
            k
        );
    }

    /// Sanity check: when `every` aligns such that the valid QR frame is the
    /// one skipped, the loop never sees the header and reports `0/0 blocks`.
    /// Together with the previous test, this proves `--every` actually drops
    /// frames rather than being a no-op.
    #[test]
    fn run_pipe_decode_every_can_skip_valid_frames() {
        let plaintext = vec![0xBBu8; DEFAULT_BLOCK_SIZE * 4];
        let (_privkey, pubkey) = crypto::keygen();
        let encrypted = crypto::encrypt(&plaintext, &pubkey).unwrap();
        let encrypted_size = encrypted.len() as u32;
        let source_blocks = split_into_blocks(&encrypted, DEFAULT_BLOCK_SIZE).unwrap();

        let block = encode_block(&source_blocks, 0, DEFAULT_BLOCK_SIZE);
        let frame_bytes = encode_frame(&block, encrypted_size).unwrap();
        let code = QrCode::with_error_correction_level(&frame_bytes, EcLevel::M).unwrap();
        let (rgba_qr, w, h) = render_qr_rgba(&code, 4);
        let bgra_qr = rgba_to_bgra(&rgba_qr);
        let bgra_garbage = vec![255u8; (w * h * 4) as usize];

        // Order: [valid_qr, garbage]. With every=2, frame 1 (the QR) is
        // skipped and frame 2 (garbage) decodes to nothing — the loop
        // never sees a header and reports `0/0 blocks`.
        let mut stream: Vec<u8> = Vec::new();
        stream.extend_from_slice(&bgra_qr);
        stream.extend_from_slice(&bgra_garbage);

        let err = run_pipe_decode(stream.as_slice(), w, h, 2).unwrap_err();
        assert!(
            err.to_string().contains("0/0 blocks"),
            "every=2 starting on a valid frame should skip it; got: {}",
            err
        );
    }
}
