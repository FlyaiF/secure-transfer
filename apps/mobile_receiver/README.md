# Visual Transfer Mobile Receiver

Android-first Flutter receiver for Visual Transfer.

## Current Scope

This app provides the v1 mobile receiver shell:

- manual paste/type private-key entry
- Android camera QR scanning through `mobile_scanner`
- binary QR payload extraction
- Dart FFI bridge to the Rust mobile receive core
- receive progress UI
- save/share UI for completed files

The Rust mobile receive core is implemented in `../../crates/mobile-core` and is
built into Android native libraries during `flutter build apk`.

## Checks

Run from this directory:

```bash
flutter pub get
flutter analyze
flutter test
flutter build apk --debug
```

If `flutter build apk --debug` fails in `JdkImageTransform` while invoking
`jlink` from a GraalVM/GraalCE JDK, run it with a standard OpenJDK 17:

```bash
JAVA_HOME=$(/usr/libexec/java_home -v 17.0.2) flutter build apk --debug
```

The APK build also requires these Rust Android targets:

```bash
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
```

Run from the repository root:

```bash
cargo test --all --locked
```
