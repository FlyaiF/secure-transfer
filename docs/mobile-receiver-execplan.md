# Add an Android Flutter Receiver for Camera-Based Visual Transfer

This ExecPlan is a living document. Keep Progress, Surprises & Discoveries,
Decision Log, and Outcomes & Retrospective up to date as work proceeds.

## Purpose / Big Picture

After this change, a user should be able to receive a file on an Android phone
by opening a Flutter app, pasting the receiver private key, pointing the rear
camera at the sender terminal QR codes, watching receive progress, and saving
the reconstructed decrypted file.

The desktop sender remains unchanged. The mobile app uses Android-native QR
scanning and passes decoded QR payload bytes into Rust, where the transfer
protocol, fountain reconstruction, and decryption stay centralized.

## Progress

- [x] (2026-06-06) Confirmed the repository is a Rust workspace with sender,
  receiver, and common protocol crates.
- [x] (2026-06-06) Added `crates/mobile-core`, a Rust library that accepts
  decoded QR payload bytes and returns progress or completed file bytes.
- [x] (2026-06-06) Added public-key derivation from a pasted private key.
- [x] (2026-06-06) Added an Android-only Flutter app skeleton at
  `apps/mobile_receiver`.
- [x] (2026-06-06) Added native QR scanning UI, manual private-key validation,
  progress display, and save/share UI.
- [x] (2026-06-06) Extended CI with a separate `mobile-android` job.
- [x] (2026-06-08) Verified Android debug APK build succeeds locally when
  Gradle runs with standard OpenJDK 17 instead of GraalCE.
- [x] (2026-06-08) Packaged the Rust mobile core as Android native libraries
  and connected Flutter to it through Dart FFI.
- [ ] Validate end-to-end on a real Android device against `transfer-sender`.

## Surprises & Discoveries

- The receiver decode state was already separated from desktop capture.
  `DecodeState::process_frame` in `crates/receiver/src/decode.rs` accepts image
  frames independently of `xcap`.
- The selected Flutter scanner package exposes decoded QR bytes through
  `rawDecodedBytes`, which is the right boundary for this design.
- The Flutter app now packages `libtransfer_mobile_core.so` for `armeabi-v7a`,
  `arm64-v8a`, and `x86_64` during the Gradle build. The generated libraries
  live under the app build directory and should not be committed.
- Local Android builds can fail in `JdkImageTransform` if Gradle uses GraalCE's
  `jlink`. Running the build with standard OpenJDK 17 resolves that local
  toolchain failure.

## Decision Log

- Decision: Build Android first.
  Rationale: Android-first keeps the first mobile version focused and avoids
  solving iOS packaging, camera, and file-save differences before the receive
  flow is proven.
  Date/Author: 2026-06-06 / User + Codex

- Decision: Use native mobile QR scanning in v1.
  Rationale: Android-native scanning is likely faster to integrate and more
  reliable with real phone cameras. Decoded QR payload bytes still flow into
  Rust for protocol handling, fountain reconstruction, and decryption.
  Date/Author: 2026-06-06 / User + Codex

- Decision: Private key entry is manual paste/type in v1.
  Rationale: The private key is sensitive. Manual input avoids adding a
  camera-based key import flow before the core receive path is proven.
  Date/Author: 2026-06-06 / User

- Decision: CI builds an Android debug APK only, not signed release artifacts.
  Rationale: The first mobile milestone needs proof that the app builds and
  tests on every PR. Release signing and store distribution add secrets and
  process work that do not prove the receiver flow.
  Date/Author: 2026-06-06 / User + Codex

## Outcomes & Retrospective

The branch now contains the Rust mobile receive core, Android Flutter scaffold,
and CI coverage for analysis, tests, and debug APK builds. The remaining
runtime-critical task is Android native library packaging and the Flutter/Rust
bridge implementation.

## Context and Orientation

Important files:

- `crates/common/src/crypto.rs` contains encryption, decryption, key generation,
  and public-key derivation.
- `crates/common/src/fountain.rs` contains fountain encoding and decoding.
- `crates/common/src/protocol.rs` contains QR frame encoding and decoding.
- `crates/mobile-core/src/lib.rs` contains the mobile-facing Rust receive
  session.
- `apps/mobile_receiver/lib/main.dart` contains the Flutter receiver UI.
- `.github/workflows/ci.yml` contains Rust CI and Android mobile CI.

## Plan of Work

The Rust mobile core exposes a small C ABI from `transfer-mobile-core`. The
Flutter app builds Android `.so` files for the required ABIs and packages them
through the Android build. The Dart FFI wrapper opens the packaged native
library and maps these calls:

    create_session(private_key_base64) -> session handle + public key
    feed_qr_payload(session handle, bytes) -> progress or completed file bytes
    free_session(session handle)

Next, run the existing sender on a desktop and validate the Android app on a
real phone. The acceptance test is receiving both a small text file and a larger
binary file and confirming byte-for-byte equality.

## Validation and Acceptance

Run from the repository root:

    cargo fmt --all -- --check
    cargo test --all --locked

Run from `apps/mobile_receiver`:

    flutter analyze
    flutter test
    flutter build apk --debug

If the APK build fails in `JdkImageTransform` with a GraalVM/GraalCE `jlink`,
retry with standard OpenJDK 17:

    JAVA_HOME=$(/usr/libexec/java_home -v 17.0.2) flutter build apk --debug

The APK build requires these Rust Android targets:

    rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android

The full feature is accepted only after a real Android device can receive and
save a file from the existing desktop sender.

## Idempotence and Recovery

Do not remove the existing desktop sender or receiver paths. If Android native
packaging blocks, keep `crates/mobile-core` independently tested by Rust CI and
keep the Flutter app building with the unavailable-core fallback.

If camera scanning is unreliable, tune `mobile_scanner` detection timeout,
sender FPS, QR error correction, and sender block size before considering a
Rust image-decoding path on mobile.
