use image::GrayImage;
use qrcode::QrCode;

use transfer_common::crypto;
use transfer_common::fountain::{encode_block, split_into_blocks, Decoder, BLOCK_SIZE};
use transfer_common::protocol::{decode_frame, encode_frame};

/// Render a QR code to a grayscale image for testing.
fn qr_to_gray_image(code: &QrCode) -> GrayImage {
    let width = code.width();
    let scale = 4u32; // 4 pixels per module
    let img_size = width as u32 * scale + 8 * scale; // quiet zone
    let mut img = GrayImage::from_pixel(img_size, img_size, image::Luma([255u8]));

    for y in 0..width {
        for x in 0..width {
            if code[(x, y)] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let px = (x as u32 + 4) * scale + dx;
                        let py = (y as u32 + 4) * scale + dy;
                        img.put_pixel(px, py, image::Luma([0u8]));
                    }
                }
            }
        }
    }
    img
}

/// Decode QR from grayscale image (same logic as receiver).
fn decode_qr(image: &GrayImage) -> Option<Vec<u8>> {
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

#[test]
fn test_full_pipeline_small_file() {
    let original_data = b"Hello, visual transfer! This is a small test file.";
    full_pipeline_test(original_data);
}

#[test]
fn test_full_pipeline_exact_block_size() {
    let original_data = vec![0x42u8; BLOCK_SIZE];
    full_pipeline_test(&original_data);
}

#[test]
fn test_full_pipeline_multi_block() {
    // ~5 blocks worth of data
    let original_data: Vec<u8> = (0..BLOCK_SIZE * 5).map(|i| (i % 256) as u8).collect();
    full_pipeline_test(&original_data);
}

fn full_pipeline_test(original_data: &[u8]) {
    // 1. Generate keypair
    let (privkey, pubkey) = crypto::keygen();

    // 2. Encrypt
    let encrypted = crypto::encrypt(original_data, &pubkey).unwrap();
    let encrypted_size = encrypted.len() as u32;

    // 3. Split into fountain source blocks
    let source_blocks = split_into_blocks(&encrypted).unwrap();
    let k = source_blocks.len();

    // 4. Encode → QR → image → decode QR → fountain decode
    let mut decoder = Decoder::new(k as u16);
    let mut seed = 0u32;
    let mut qr_success = 0u32;

    while !decoder.is_complete() {
        // Generate fountain block
        let block = encode_block(&source_blocks, seed);
        let frame_data = encode_frame(&block, encrypted_size);

        // Encode to QR
        let code = QrCode::with_error_correction_level(&frame_data, qrcode::EcLevel::M)
            .expect("QR encode failed");

        // Render to image
        let img = qr_to_gray_image(&code);

        // Decode QR from image
        if let Some(decoded_data) = decode_qr(&img) {
            // Parse protocol frame
            if let Some(frame) = decode_frame(&decoded_data) {
                assert_eq!(frame.encrypted_size, encrypted_size);
                assert_eq!(frame.block.total_blocks, k as u16);
                assert_eq!(frame.block.seed, seed);
                decoder.add_block(&frame.block);
                qr_success += 1;
            }
        }

        seed += 1;
        if seed > (k as u32) * 30 {
            panic!(
                "too many attempts: {} seeds tried, {} QR decoded, {}/{} blocks decoded",
                seed,
                qr_success,
                decoder.decoded_count(),
                k
            );
        }
    }

    // 5. Reassemble
    let reassembled = decoder.reassemble(encrypted_size as usize).unwrap();
    assert_eq!(reassembled.len(), encrypted.len());

    // 6. Decrypt
    let decrypted = crypto::decrypt(&reassembled, &privkey).unwrap();
    assert_eq!(decrypted, original_data);

    eprintln!(
        "Pipeline test passed: {} bytes, {} source blocks, {} encoded blocks needed, {} QR round-trips",
        original_data.len(),
        k,
        seed,
        qr_success
    );
}
