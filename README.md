# Visual Transfer

Transfer files between machines by encoding data as QR codes on screen. The sender displays QR codes in a terminal, and the receiver captures and decodes them through any screen-sharing channel (RDP, VNC, video call, etc.).

No direct network connection needed between the machines — if you can see the screen, you can transfer data.

## Features

- **Fountain codes (LT codes)** — no sync needed, tolerates dropped/duplicate frames
- **End-to-end encryption** — X25519 ECDH + AES-256-GCM with forward secrecy
- **Terminal QR rendering** — works in any terminal, no GUI required
- **Cross-platform** — pure Rust, builds on Linux / macOS / Windows
- **Built-in screen capture** — auto-spawns ffmpeg, no manual piping

## How It Works

```
 Remote Machine                    Local Machine
┌────────────────┐                ┌──────────────────┐
│  sender send   │  any screen    │  receiver screen │
│  (QR in term)  │ ── sharing ──> │  (auto capture)  │
└────────────────┘   channel      └──────────────────┘
```

1. File is encrypted with an ephemeral key (only the receiver's private key can decrypt)
2. Encrypted data is split into blocks and fountain-encoded (LT codes)
3. Each encoded block is displayed as a QR code, cycling endlessly
4. Receiver captures screen, decodes QR codes, and collects fountain blocks
5. Once enough blocks are collected (~5% overhead), the file is reassembled and decrypted

## Build

```bash
cargo build --release
```

Binaries are at `target/release/transfer-sender` and `target/release/transfer-receiver`.

No native library dependencies. Just needs `ffmpeg` installed on the receiving machine.

## Usage

### Step 1: Generate keypair (local machine, one time)

```bash
./transfer-receiver keygen --out ~/.transfer_key
# Prints: aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789abcdef=
#         ↑ This is your PUBLIC key. Give it to the sender.
```

### Step 2: Send file (remote machine)

```bash
./transfer-sender send /path/to/secret.pdf \
  --pubkey aBcDeFgHiJkLmNoPqRsTuVwXyZ0123456789abcdef=
```

QR codes start cycling in the terminal. Leave it running.

### Step 3: Receive file (local machine)

Open whatever shows the remote screen (RDP, VNC, video call), then:

```bash
./transfer-receiver screen --privkey ~/.transfer_key -o received.pdf
```

The receiver auto-launches ffmpeg, captures your screen, finds the QR codes, and shows progress:

```
Starting ffmpeg: ffmpeg -f x11grab -framerate 5 -i :0 -vf scale=1280:720 -f rawvideo -pix_fmt bgra pipe:1
First frame received! 42 source blocks, 5440 bytes encrypted
Received: 45 unique | Decoded: 42/42 (100%) | 14.8s
All blocks received!
File saved to: received.pdf (5120 bytes)
```

Done. Three commands total.

## Same-Machine Quick Test

```bash
# Terminal 1: generate key and start receiver
./transfer-receiver keygen --out ~/.transfer_key
# copy the public key output

# Terminal 2: start sender
echo "Hello, world!" > /tmp/test.txt
./transfer-sender send /tmp/test.txt --pubkey <paste-public-key>

# Terminal 1: receive
./transfer-receiver screen --privkey ~/.transfer_key -o /tmp/received.txt

# Verify
diff /tmp/test.txt /tmp/received.txt
```

## All Commands

### Sender

```
transfer-sender send <FILE> --pubkey <BASE64>
    --fps <N>           Frames per second (default: 2)
    --ec <LEVEL>        QR error correction: L, M, Q, H (default: M)
    --block-size <N>    Fountain block size in bytes (default: 128).
                        Larger values pack more data per QR but produce
                        denser codes that need higher-resolution scanning.
                        Receiver auto-adapts from the frame header.
```

### Receiver

```
# Generate keypair
transfer-receiver keygen [--out <PATH>]

# Capture screen automatically (spawns ffmpeg internally)
transfer-receiver screen --privkey <PATH> [-o <FILE>]
    --width <W>     Capture width (default: 1280)
    --height <H>    Capture height (default: 720)
    --fps <N>       Capture framerate (default: 5)
    --every <N>     Decode every Nth frame (default: 1)

# Manual pipe mode (if you want to control ffmpeg yourself)
transfer-receiver pipe --privkey <PATH> --width <W> --height <H> [-o <FILE>]
    --every <N>     Decode every Nth frame (default: 1)

# Decode from saved image files
transfer-receiver decode --privkey <PATH> -o <FILE> <IMAGES...>
```

## Tips

- **Slow connection?** Lower `--fps 1` on the sender for bigger QR codes per frame
- **Bad video quality?** Use `--ec H` on the sender for max error correction
- **High CPU on receiver?** Use `--every 3` to only decode every 3rd frame
- **No ffmpeg?** Take screenshots manually and use `receiver decode` mode

## Platform Notes

The receiver auto-detects your platform for ffmpeg:
- **Linux**: uses `-f x11grab` (X11). Wayland falls back to XWayland.
- **macOS**: uses `-f avfoundation`
- **Windows**: uses `-f gdigrab`

For manual control, use `receiver pipe` and run ffmpeg yourself.

## Security

- **Asymmetric encryption**: sender only needs the receiver's public key
- **Forward secrecy**: each transfer uses a fresh ephemeral X25519 keypair
- **Compromising the remote machine** reveals only the public key (useless for decryption)
- **Captured QR stream** is AES-256-GCM encrypted noise without the private key
- **GCM authentication tag** detects wrong keys or tampered data

## Technical Details

- Block size: 128 bytes per fountain source block (default; tunable via `--block-size`)
- Protocol header: 15 bytes per QR frame
- Encryption overhead: 60 bytes total (32B ephemeral pubkey + 12B nonce + 16B GCM tag)
- Typical QR version: 10-15 (auto-selected based on payload size)
- Fountain overhead: ~5-10% more blocks than source blocks needed
- Throughput: ~1-3 KB/s depending on QR size, FPS, and capture quality

## Requirements

- **Rust 1.70+** to build
- **ffmpeg** on the receiving machine (for `screen` and `pipe` modes)
- Any screen-sharing tool to see the remote terminal (RDP, VNC, SSH+tmux, video call, etc.)
