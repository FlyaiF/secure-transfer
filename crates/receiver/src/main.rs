use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use clap::{Parser, Subcommand};
use image::GrayImage;

use transfer_common::crypto;
use transfer_common::fountain::Decoder;
use transfer_common::protocol;

#[derive(Parser)]
#[command(name = "receiver", about = "Visual data transfer - receiver side")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Generate a new keypair
    Keygen {
        /// Path to save the private key
        #[arg(long, default_value = "~/.transfer_key")]
        out: String,
    },

    /// Receive by capturing the screen (auto-detects platform, uses ffmpeg)
    Screen {
        /// Path to the private key file
        #[arg(long)]
        privkey: String,

        /// Output file path
        #[arg(long, short)]
        output: Option<String>,

        /// Capture resolution width
        #[arg(long, default_value = "1280")]
        width: u32,

        /// Capture resolution height
        #[arg(long, default_value = "720")]
        height: u32,

        /// Capture framerate
        #[arg(long, default_value = "5")]
        fps: u32,

        /// Only decode every Nth frame (reduce CPU)
        #[arg(long, default_value = "1")]
        every: u32,
    },

    /// Receive by reading raw BGRA frames from stdin
    Pipe {
        /// Path to the private key file
        #[arg(long)]
        privkey: String,

        /// Output file path
        #[arg(long, short)]
        output: Option<String>,

        /// Frame width in pixels
        #[arg(long)]
        width: u32,

        /// Frame height in pixels
        #[arg(long)]
        height: u32,

        /// Only decode every Nth frame (reduce CPU)
        #[arg(long, default_value = "1")]
        every: u32,
    },

    /// Decode QR codes from image files
    Decode {
        /// Path to the private key file
        #[arg(long)]
        privkey: String,

        /// Output file path
        #[arg(long, short)]
        output: String,

        /// Input image files containing QR codes
        files: Vec<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Keygen { out } => keygen(&out),
        Commands::Screen {
            privkey,
            output,
            width,
            height,
            fps,
            every,
        } => receive_screen(&privkey, output, width, height, fps, every),
        Commands::Pipe {
            privkey,
            output,
            width,
            height,
            every,
        } => receive_pipe_stdin(&privkey, output, width, height, every),
        Commands::Decode {
            privkey,
            output,
            files,
        } => decode_files(&privkey, &output, &files),
    }
}

// ─── Key generation ──────────────────────────────────────────────────────────

fn keygen(out_path: &str) -> Result<()> {
    let path = expand_tilde(out_path);
    let (privkey, pubkey) = crypto::keygen();

    let privkey_b64 = BASE64.encode(privkey);
    write_private_key(&path, &privkey_b64)?;

    let pubkey_b64 = BASE64.encode(pubkey);
    eprintln!("Private key saved to: {}", path.display());
    println!("{}", pubkey_b64);
    eprintln!("Share the public key above with the sender.");

    Ok(())
}

/// Write private key file with owner-only permissions (0600 on Unix).
/// If the file already exists, chmod it to 0600 to fix any prior broad permissions.
fn write_private_key(path: &PathBuf, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("failed to write {}", path.display()))?;
        // Ensure permissions are 0600 even if the file already existed
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to set permissions on {}", path.display()))?;
        use std::io::Write;
        file.write_all(content.as_bytes())?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        fs::write(path, content)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }
}

// ─── Screen capture via ffmpeg ──────────────────────────────────────────────

fn receive_screen(
    privkey_path: &str,
    output: Option<String>,
    width: u32,
    height: u32,
    fps: u32,
    every: u32,
) -> Result<()> {
    // Build platform-specific ffmpeg command
    let ffmpeg_args = build_ffmpeg_args(width, height, fps)?;

    eprintln!("Starting ffmpeg: ffmpeg {}", ffmpeg_args.join(" "));

    let mut child = Command::new("ffmpeg")
        .args(&ffmpeg_args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start ffmpeg — is it installed and in PATH?")?;

    let stdout = child.stdout.take().unwrap();
    let result = receive_pipe(privkey_path, output, width, height, every, stdout);

    // Clean up ffmpeg
    let _ = child.kill();
    let _ = child.wait();

    result
}

fn build_ffmpeg_args(width: u32, height: u32, fps: u32) -> Result<Vec<String>> {
    let scale = format!("scale={}:{}", width, height);
    let fps_str = fps.to_string();

    let (input_fmt, input_src) = if cfg!(target_os = "linux") {
        let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            eprintln!("Detected Wayland, trying x11grab via XWayland...");
        }
        ("x11grab".to_string(), display)
    } else if cfg!(target_os = "macos") {
        ("avfoundation".to_string(), "1".to_string())
    } else if cfg!(target_os = "windows") {
        ("gdigrab".to_string(), "desktop".to_string())
    } else {
        anyhow::bail!("unsupported platform — use 'pipe' mode with manual ffmpeg command");
    };

    Ok(vec![
        "-f".into(), input_fmt,
        "-framerate".into(), fps_str,
        "-i".into(), input_src,
        "-vf".into(), scale,
        "-f".into(), "rawvideo".into(),
        "-pix_fmt".into(), "bgra".into(),
        "pipe:1".into(),
    ])
}

// ─── Pipe mode ──────────────────────────────────────────────────────────────

fn receive_pipe_stdin(
    privkey_path: &str,
    output: Option<String>,
    width: u32,
    height: u32,
    every: u32,
) -> Result<()> {
    let stdin = std::io::stdin().lock();
    eprintln!("Reading {}x{} BGRA frames from stdin...", width, height);
    receive_pipe(privkey_path, output, width, height, every, stdin)
}

fn receive_pipe<R: Read>(
    privkey_path: &str,
    output: Option<String>,
    width: u32,
    height: u32,
    every: u32,
    mut reader: R,
) -> Result<()> {
    if width == 0 || height == 0 {
        anyhow::bail!("--width and --height must be greater than 0");
    }
    let privkey = load_privkey(privkey_path)?;
    let frame_size = (width as usize) * (height as usize) * 4;
    let mut buf = vec![0u8; frame_size];
    let mut state = DecodeState::new();
    let mut frame_num = 0u32;
    let start = Instant::now();

    loop {
        if reader.read_exact(&mut buf).is_err() {
            eprintln!("\nEnd of input stream");
            break;
        }

        frame_num += 1;
        if every > 1 && frame_num % every != 0 {
            continue;
        }

        let gray = bgra_to_gray(&buf, width, height);
        if let Some(encrypted) = state.process_frame(&gray, start.elapsed()) {
            return finalize(encrypted, &privkey, output);
        }
    }

    anyhow::bail!(
        "stream ended before transfer complete: {}/{} blocks decoded",
        state.decoded_count(),
        state.total_blocks()
    );
}

// ─── File decode mode ───────────────────────────────────────────────────────

fn decode_files(privkey_path: &str, output: &str, files: &[String]) -> Result<()> {
    let privkey = load_privkey(privkey_path)?;
    let mut state = DecodeState::new();
    let start = Instant::now();

    for file in files {
        let img = image::open(file)
            .with_context(|| format!("failed to open {}", file))?
            .to_luma8();

        if let Some(encrypted) = state.process_frame(&img, start.elapsed()) {
            return finalize(encrypted, &privkey, Some(output.to_string()));
        }
    }

    if state.total_blocks() == 0 {
        anyhow::bail!("no valid QR frames found in any input file");
    }
    anyhow::bail!(
        "incomplete: decoded {}/{} source blocks",
        state.decoded_count(),
        state.total_blocks()
    );
}

// ─── Shared decode logic ────────────────────────────────────────────────────

struct DecodeState {
    decoder: Option<Decoder>,
    encrypted_size: Option<u32>,
    unique_count: u32,
}

impl DecodeState {
    fn new() -> Self {
        Self {
            decoder: None,
            encrypted_size: None,
            unique_count: 0,
        }
    }

    fn decoded_count(&self) -> usize {
        self.decoder.as_ref().map_or(0, |d| d.decoded_count())
    }

    fn total_blocks(&self) -> usize {
        self.decoder.as_ref().map_or(0, |d| d.total_blocks())
    }

    fn process_frame(&mut self, gray: &GrayImage, elapsed: Duration) -> Option<Vec<u8>> {
        let data = decode_qr_from_image(gray)?;
        let frame = protocol::decode_frame(&data)?;

        if self.decoder.is_none() {
            eprintln!(
                "First frame received! {} source blocks, {} bytes encrypted",
                frame.block.total_blocks, frame.encrypted_size
            );
            self.decoder = Some(Decoder::new(frame.block.total_blocks));
            self.encrypted_size = Some(frame.encrypted_size);
        }

        // Reject frames from a different transfer (e.g. sender restarted with different file)
        let expected_enc_size = self.encrypted_size.unwrap();
        let expected_blocks = self.decoder.as_ref().unwrap().total_blocks() as u16;
        if frame.encrypted_size != expected_enc_size || frame.block.total_blocks != expected_blocks {
            // Silently skip mismatched frames
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

fn finalize(encrypted: Vec<u8>, privkey: &[u8; 32], output: Option<String>) -> Result<()> {
    eprintln!("Decrypting...");
    let plaintext = crypto::decrypt(&encrypted, privkey)
        .map_err(|e| anyhow::anyhow!("Decryption failed: {}", e))?;
    let out_path = output.unwrap_or_else(|| "received_file".to_string());
    fs::write(&out_path, &plaintext)?;
    eprintln!("File saved to: {} ({} bytes)", out_path, plaintext.len());
    Ok(())
}

// ─── Utilities ──────────────────────────────────────────────────────────────

fn load_privkey(path: &str) -> Result<[u8; 32]> {
    let path = expand_tilde(path);
    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read private key from {}", path.display()))?;
    let bytes = BASE64
        .decode(contents.trim())
        .context("invalid base64 private key")?;
    bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("private key must be 32 bytes"))
}

fn decode_qr_from_image(image: &GrayImage) -> Option<Vec<u8>> {
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

fn bgra_to_gray(bgra: &[u8], w: u32, h: u32) -> GrayImage {
    GrayImage::from_fn(w, h, |x, y| {
        let i = ((y * w + x) * 4) as usize;
        let b = bgra[i] as f32;
        let g = bgra[i + 1] as f32;
        let r = bgra[i + 2] as f32;
        let luma = (0.299 * r + 0.587 * g + 0.114 * b) as u8;
        image::Luma([luma])
    })
}

fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Some(home) = dirs_path() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

fn dirs_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
