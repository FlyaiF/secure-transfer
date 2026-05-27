use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use clap::{Parser, Subcommand};

use transfer_common::crypto;

mod capture;
mod decode;

use decode::{run_pipe_decode, DecodeState};

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

    /// Receive by capturing a monitor (in-process, no ffmpeg required)
    Screen {
        /// Path to the private key file
        #[arg(long)]
        privkey: String,

        /// Output file path
        #[arg(long, short)]
        output: Option<String>,

        /// Monitor selector: an index (0, 1, …) or a name substring.
        /// Defaults to the primary monitor.
        #[arg(long)]
        monitor: Option<String>,

        /// Downscale captured frames to this width before QR decode
        #[arg(long, default_value = "1280")]
        width: u32,

        /// Downscale captured frames to this height before QR decode
        #[arg(long, default_value = "720")]
        height: u32,

        /// Target capture framerate (hint to the OS)
        #[arg(long, default_value = "5")]
        fps: u32,

        /// Only decode every Nth frame (reduce CPU)
        #[arg(long, default_value = "1")]
        every: u32,
    },

    /// Receive by capturing a specific window (in-process, no ffmpeg required)
    Window {
        /// Path to the private key file
        #[arg(long)]
        privkey: String,

        /// Output file path
        #[arg(long, short)]
        output: Option<String>,

        /// Window title substring to match (case-insensitive)
        #[arg(long, conflicts_with = "id")]
        title: Option<String>,

        /// Exact window id (from `xcap`'s enumeration)
        #[arg(long)]
        id: Option<u32>,

        /// Downscale captured frames to this width before QR decode
        #[arg(long, default_value = "1280")]
        width: u32,

        /// Downscale captured frames to this height before QR decode
        #[arg(long, default_value = "720")]
        height: u32,

        /// Capture framerate (xcap doesn't stream windows yet, so we poll)
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
            monitor,
            width,
            height,
            fps,
            every,
        } => capture::receive_screen(&privkey, output, monitor, width, height, fps, every),
        Commands::Window {
            privkey,
            output,
            title,
            id,
            width,
            height,
            fps,
            every,
        } => capture::receive_window(&privkey, output, title, id, width, height, fps, every),
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
        // Best-effort on Windows: write the file plainly.
        let _ = content;
        fs::write(path, content).with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }
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
    let encrypted = run_pipe_decode(stdin, width, height, every)?;
    finalize_transfer(encrypted, privkey_path, output)
}

// ─── File decode mode ───────────────────────────────────────────────────────

fn decode_files(privkey_path: &str, output: &str, files: &[String]) -> Result<()> {
    let privkey = load_privkey(privkey_path)?;
    let mut state = DecodeState::new();
    let start = std::time::Instant::now();

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

// ─── Finalization ───────────────────────────────────────────────────────────

/// Shared entry point used by capture-driven modes.
pub(crate) fn finalize_transfer(
    encrypted: Vec<u8>,
    privkey_path: &str,
    output: Option<String>,
) -> Result<()> {
    let privkey = load_privkey(privkey_path)?;
    finalize(encrypted, &privkey, output)
}

fn finalize(encrypted: Vec<u8>, privkey: &[u8; 32], output: Option<String>) -> Result<()> {
    eprintln!("Decrypting...");
    let plaintext = crypto::decrypt(encrypted.as_slice(), privkey)
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

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs_path() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

fn dirs_path() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}
