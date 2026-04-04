use std::fs;
use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use clap::{Parser, Subcommand};
use crossterm::{
    cursor,
    style::{self, Stylize},
    terminal, ExecutableCommand, QueueableCommand,
};
use qrcode::QrCode;

use transfer_common::crypto;
use transfer_common::fountain::{encode_block, split_into_blocks};
use transfer_common::protocol::encode_frame;

#[derive(Parser)]
#[command(name = "sender", about = "Visual data transfer - sender side")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Send a file by displaying QR codes
    Send {
        /// Path to the file to send
        file: String,

        /// Receiver's public key (base64)
        #[arg(long)]
        pubkey: String,

        /// Frames per second
        #[arg(long, default_value = "2")]
        fps: f64,

        /// QR error correction level: L, M, Q, H
        #[arg(long, default_value = "M")]
        ec: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Send {
            file,
            pubkey,
            fps,
            ec,
        } => send_file(&file, &pubkey, fps, &ec),
    }
}

fn send_file(path: &str, pubkey_b64: &str, fps: f64, ec_level: &str) -> Result<()> {
    // Decode public key
    let pubkey_bytes = BASE64.decode(pubkey_b64).context("invalid base64 public key")?;
    let pubkey: [u8; 32] = pubkey_bytes
        .try_into()
        .map_err(|_| anyhow::anyhow!("public key must be 32 bytes"))?;

    // Read and encrypt file
    let data = fs::read(path).with_context(|| format!("failed to read {}", path))?;
    let file_size = data.len();
    eprintln!("File: {} ({} bytes)", path, file_size);

    let encrypted = crypto::encrypt(&data, &pubkey).map_err(|e| anyhow::anyhow!(e))?;
    let encrypted_size = encrypted.len() as u32;
    eprintln!("Encrypted: {} bytes", encrypted_size);

    // Split into fountain source blocks
    let source_blocks = split_into_blocks(&encrypted).map_err(|e| anyhow::anyhow!(e))?;
    let k = source_blocks.len();
    eprintln!("Source blocks: {} (block size: 128 bytes)", k);

    // Parse EC level
    let ec = match ec_level.to_uppercase().as_str() {
        "L" => qrcode::EcLevel::L,
        "M" => qrcode::EcLevel::M,
        "Q" => qrcode::EcLevel::Q,
        "H" => qrcode::EcLevel::H,
        _ => anyhow::bail!("invalid EC level: {}", ec_level),
    };

    if fps <= 0.0 || !fps.is_finite() {
        anyhow::bail!("--fps must be a positive number, got {}", fps);
    }
    let interval = Duration::from_secs_f64(1.0 / fps);

    // Set up terminal
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    stdout.execute(terminal::EnterAlternateScreen)?;
    stdout.execute(cursor::Hide)?;

    let result = run_display_loop(&mut stdout, &source_blocks, encrypted_size, ec, interval, k);

    // Restore terminal
    stdout.execute(cursor::Show)?;
    stdout.execute(terminal::LeaveAlternateScreen)?;
    terminal::disable_raw_mode()?;

    result
}

fn run_display_loop(
    stdout: &mut io::Stdout,
    source_blocks: &[Vec<u8>],
    encrypted_size: u32,
    ec: qrcode::EcLevel,
    interval: Duration,
    k: usize,
) -> Result<()> {
    let mut seed = 0u32;

    loop {
        // Check for 'q' key to quit
        if crossterm::event::poll(Duration::from_millis(0))? {
            if let crossterm::event::Event::Key(key) = crossterm::event::read()? {
                if key.code == crossterm::event::KeyCode::Char('q')
                    || key.code == crossterm::event::KeyCode::Esc
                {
                    break;
                }
            }
        }

        // Generate fountain-encoded block
        let block = encode_block(source_blocks, seed);
        let frame_data = encode_frame(&block, encrypted_size);

        // Encode as QR code
        let code = QrCode::with_error_correction_level(&frame_data, ec)
            .with_context(|| format!("QR encode failed for seed {} ({} bytes payload)", seed, frame_data.len()))?;

        // Render to terminal
        render_qr_terminal(stdout, &code, seed, k)?;

        seed = seed.wrapping_add(1);
        thread::sleep(interval);
    }

    Ok(())
}

/// Render a QR code to the terminal using Unicode half-block characters.
/// ▀ (upper half block), ▄ (lower half block), █ (full block), ' ' (space)
fn render_qr_terminal(stdout: &mut io::Stdout, code: &QrCode, seed: u32, k: usize) -> Result<()> {
    let width = code.width();
    let colors: Vec<Vec<bool>> = (0..width)
        .map(|y| {
            (0..width)
                .map(|x| code[(x as usize, y as usize)] == qrcode::Color::Dark)
                .collect()
        })
        .collect();

    let (term_w, term_h) = terminal::size()?;

    // QR needs width/2 + 2 columns (quiet zone), height/2 + 2 rows
    let qr_cols = width as u16 + 4; // 2 quiet zone on each side
    let qr_rows = ((width + 1) / 2) as u16 + 3; // half-block + quiet zone + status line

    let start_col = term_w.saturating_sub(qr_cols) / 2;
    let start_row = term_h.saturating_sub(qr_rows) / 2;

    stdout.queue(terminal::Clear(terminal::ClearType::All))?;

    // Render QR using half-block characters (2 rows per character line)
    for row_pair in (0..width).step_by(2) {
        let screen_row = start_row + (row_pair / 2) as u16 + 1; // +1 for quiet zone
        stdout.queue(cursor::MoveTo(start_col, screen_row))?;

        // Quiet zone left
        stdout.queue(style::PrintStyledContent("  ".on_white()))?;

        for col in 0..width {
            let top = colors[row_pair][col];
            let bottom = if row_pair + 1 < width {
                colors[row_pair + 1][col]
            } else {
                false // quiet zone for odd-height QR
            };

            let ch = match (top, bottom) {
                (true, true) => "█".black().on_black(),
                (true, false) => "▀".black().on_white(),
                (false, true) => "▄".black().on_white(),
                (false, false) => " ".on_white(),
            };
            stdout.queue(style::PrintStyledContent(ch))?;
        }

        // Quiet zone right
        stdout.queue(style::PrintStyledContent("  ".on_white()))?;
    }

    // Quiet zone top (draw before QR)
    let top_row = start_row;
    stdout.queue(cursor::MoveTo(start_col, top_row))?;
    let quiet_line = " ".repeat(qr_cols as usize);
    stdout.queue(style::PrintStyledContent(quiet_line.as_str().on_white()))?;

    // Quiet zone bottom
    let bottom_row = start_row + ((width + 1) / 2) as u16 + 1;
    stdout.queue(cursor::MoveTo(start_col, bottom_row))?;
    let quiet_line = " ".repeat(qr_cols as usize);
    stdout.queue(style::PrintStyledContent(quiet_line.as_str().on_white()))?;

    // Status line at bottom
    let status = format!(
        " Seed: {} | Blocks: {} | Press 'q' to quit ",
        seed, k
    );
    stdout.queue(cursor::MoveTo(0, term_h - 1))?;
    stdout.queue(style::PrintStyledContent(status.as_str().white().on_dark_blue()))?;

    stdout.flush()?;
    Ok(())
}
