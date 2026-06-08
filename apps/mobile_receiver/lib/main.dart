import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

import 'package:ffi/ffi.dart';
import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';
import 'package:path_provider/path_provider.dart';
import 'package:share_plus/share_plus.dart';

void main() {
  runApp(const ReceiverApp());
}

class ReceiverApp extends StatelessWidget {
  const ReceiverApp({super.key, this.core});

  final ReceiverCore? core;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Visual Transfer',
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(
          seedColor: const Color(0xFF0F766E),
          brightness: Brightness.light,
        ),
        useMaterial3: true,
      ),
      home: ReceiverHome(core: core ?? const NativeReceiverCore()),
    );
  }
}

class ReceiverHome extends StatefulWidget {
  const ReceiverHome({super.key, required this.core});

  final ReceiverCore core;

  @override
  State<ReceiverHome> createState() => _ReceiverHomeState();
}

class _ReceiverHomeState extends State<ReceiverHome> {
  final TextEditingController _privateKeyController = TextEditingController();
  ReceiverSession? _session;
  String? _keyError;
  String? _status;

  @override
  void dispose() {
    _session?.dispose();
    _privateKeyController.dispose();
    super.dispose();
  }

  Future<void> _start() async {
    final privateKey = _privateKeyController.text.trim();
    if (!_looksLikePrivateKey(privateKey)) {
      setState(() {
        _keyError = 'Private key must be base64 text for exactly 32 bytes.';
      });
      return;
    }

    try {
      final session = await widget.core.createSession(privateKey);
      setState(() {
        _session = session;
        _keyError = null;
        _status = 'Ready to scan';
      });
    } on Object catch (error) {
      setState(() {
        _keyError = null;
        _status = error.toString();
      });
    }
  }

  bool _looksLikePrivateKey(String value) {
    try {
      return base64.decode(value).length == 32;
    } on FormatException {
      return false;
    }
  }

  @override
  Widget build(BuildContext context) {
    final session = _session;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Visual Transfer'),
        actions: [
          if (session != null)
            IconButton(
              tooltip: 'Reset',
              onPressed: () {
                _session?.dispose();
                setState(() {
                  _session = null;
                  _status = null;
                });
              },
              icon: const Icon(Icons.restart_alt),
            ),
        ],
      ),
      body: SafeArea(
        child: session == null
            ? _PrivateKeyPane(
                controller: _privateKeyController,
                errorText: _keyError,
                status: _status,
                onStart: _start,
              )
            : _ScannerPane(
                session: session,
                status: _status,
                onStatusChanged: (value) => setState(() => _status = value),
              ),
      ),
    );
  }
}

class _PrivateKeyPane extends StatelessWidget {
  const _PrivateKeyPane({
    required this.controller,
    required this.errorText,
    required this.status,
    required this.onStart,
  });

  final TextEditingController controller;
  final String? errorText;
  final String? status;
  final VoidCallback onStart;

  @override
  Widget build(BuildContext context) {
    return ListView(
      padding: const EdgeInsets.all(20),
      children: [
        Text('Receiver key', style: Theme.of(context).textTheme.headlineSmall),
        const SizedBox(height: 12),
        TextField(
          controller: controller,
          minLines: 3,
          maxLines: 5,
          decoration: InputDecoration(
            border: const OutlineInputBorder(),
            errorText: errorText,
            labelText: 'Private key',
          ),
          textInputAction: TextInputAction.done,
        ),
        const SizedBox(height: 16),
        FilledButton.icon(
          onPressed: onStart,
          icon: const Icon(Icons.qr_code_scanner),
          label: const Text('Start scanning'),
        ),
        if (status != null) ...[
          const SizedBox(height: 16),
          Text(status!, style: Theme.of(context).textTheme.bodyMedium),
        ],
      ],
    );
  }
}

class _ScannerPane extends StatefulWidget {
  const _ScannerPane({
    required this.session,
    required this.status,
    required this.onStatusChanged,
  });

  final ReceiverSession session;
  final String? status;
  final ValueChanged<String> onStatusChanged;

  @override
  State<_ScannerPane> createState() => _ScannerPaneState();
}

class _ScannerPaneState extends State<_ScannerPane> {
  final MobileScannerController _scannerController = MobileScannerController(
    formats: const [BarcodeFormat.qrCode],
    detectionSpeed: DetectionSpeed.normal,
    detectionTimeoutMs: 120,
    autoZoom: true,
  );
  bool _busy = false;
  ReceivedFile? _receivedFile;
  ReceiveProgress _progress = const ReceiveProgress.empty();

  @override
  void dispose() {
    _scannerController.dispose();
    super.dispose();
  }

  Future<void> _onDetect(BarcodeCapture capture) async {
    if (_busy || _receivedFile != null) {
      return;
    }

    final payload = _firstPayload(capture);
    if (payload == null) {
      return;
    }

    _busy = true;
    try {
      final update = await widget.session.feedQrPayload(payload);
      if (!mounted) {
        return;
      }
      switch (update) {
        case IgnoredQrPayload():
          widget.onStatusChanged('Looking for transfer frames');
        case ReceiveInProgress(:final progress):
          setState(() => _progress = progress);
          widget.onStatusChanged(
            'Decoded ${progress.decodedBlocks}/${progress.totalBlocks}',
          );
        case ReceiveComplete(:final progress, :final file):
          await _scannerController.stop();
          setState(() {
            _progress = progress;
            _receivedFile = file;
          });
          widget.onStatusChanged('Transfer complete');
      }
    } on Object catch (error) {
      if (mounted) {
        widget.onStatusChanged(error.toString());
      }
    } finally {
      _busy = false;
    }
  }

  Uint8List? _firstPayload(BarcodeCapture capture) {
    for (final barcode in capture.barcodes) {
      final decoded = barcode.rawDecodedBytes;
      switch (decoded) {
        case DecodedBarcodeBytes(:final bytes):
          return bytes;
        case DecodedVisionBarcodeBytes(:final bytes, :final rawBytes):
          return bytes ?? rawBytes;
        case null:
          final value = barcode.rawValue;
          if (value != null) {
            return Uint8List.fromList(utf8.encode(value));
          }
      }
    }
    return null;
  }

  Future<void> _shareReceivedFile() async {
    final file = _receivedFile;
    if (file == null) {
      return;
    }

    final directory = await getTemporaryDirectory();
    final output = File('${directory.path}/${file.name}');
    await output.writeAsBytes(file.bytes, flush: true);
    await SharePlus.instance.share(
      ShareParams(files: [XFile(output.path)], subject: 'Visual Transfer file'),
    );
  }

  @override
  Widget build(BuildContext context) {
    final receivedFile = _receivedFile;
    final progress = _progress;
    final total = progress.totalBlocks;
    final percent = total == 0 ? 0.0 : progress.decodedBlocks / total;

    return Column(
      children: [
        Expanded(
          child: Stack(
            fit: StackFit.expand,
            children: [
              MobileScanner(
                controller: _scannerController,
                onDetect: _onDetect,
              ),
              const _ScanFrame(),
            ],
          ),
        ),
        Padding(
          padding: const EdgeInsets.all(16),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              LinearProgressIndicator(value: total == 0 ? null : percent),
              const SizedBox(height: 8),
              Text(
                total == 0
                    ? 'Waiting for first frame'
                    : '${progress.decodedBlocks}/$total blocks, ${progress.uniqueBlocks} unique',
              ),
              if (widget.status != null) ...[
                const SizedBox(height: 4),
                Text(widget.status!),
              ],
              if (receivedFile != null) ...[
                const SizedBox(height: 12),
                FilledButton.icon(
                  onPressed: _shareReceivedFile,
                  icon: const Icon(Icons.ios_share),
                  label: Text('Save ${receivedFile.name}'),
                ),
              ],
            ],
          ),
        ),
      ],
    );
  }
}

class _ScanFrame extends StatelessWidget {
  const _ScanFrame();

  @override
  Widget build(BuildContext context) {
    return IgnorePointer(
      child: Center(
        child: Container(
          width: 260,
          height: 260,
          decoration: BoxDecoration(
            border: Border.all(color: Colors.white, width: 3),
            borderRadius: BorderRadius.circular(8),
          ),
        ),
      ),
    );
  }
}

abstract interface class ReceiverCore {
  Future<ReceiverSession> createSession(String privateKeyBase64);
}

abstract interface class ReceiverSession {
  String get publicKeyBase64;

  Future<ReceiveUpdate> feedQrPayload(Uint8List payload);

  void dispose();
}

class NativeReceiverCore implements ReceiverCore {
  const NativeReceiverCore();

  @override
  Future<ReceiverSession> createSession(String privateKeyBase64) async {
    final bindings = _NativeBindings.open();
    final keyBytes = Uint8List.fromList(utf8.encode(privateKeyBase64));
    final keyPtr = calloc<Uint8>(keyBytes.length);
    final errorOut = calloc<_NativeByteBuffer>();

    try {
      keyPtr.asTypedList(keyBytes.length).setAll(0, keyBytes);
      final handle = bindings.sessionNew(keyPtr, keyBytes.length, errorOut);
      if (handle == nullptr) {
        throw StateError(bindings.takeString(errorOut.ref));
      }

      final publicKey = bindings.takeString(bindings.sessionPublicKey(handle));
      return _NativeReceiverSession(
        bindings: bindings,
        handle: handle,
        publicKeyBase64: publicKey,
      );
    } finally {
      calloc.free(keyPtr);
      calloc.free(errorOut);
    }
  }
}

class _NativeReceiverSession implements ReceiverSession {
  _NativeReceiverSession({
    required this.bindings,
    required this.handle,
    required this.publicKeyBase64,
  });

  final _NativeBindings bindings;
  Pointer<Void> handle;
  bool _disposed = false;

  @override
  final String publicKeyBase64;

  @override
  Future<ReceiveUpdate> feedQrPayload(Uint8List payload) async {
    if (_disposed) {
      throw StateError('receiver session has been disposed');
    }

    final payloadPtr = calloc<Uint8>(payload.length);
    final updateOut = calloc<_NativeReceiveUpdate>();

    try {
      payloadPtr.asTypedList(payload.length).setAll(0, payload);
      final ok = bindings.sessionFeedQr(
        handle,
        payloadPtr,
        payload.length,
        updateOut,
      );

      final update = updateOut.ref;
      if (!ok || update.kind == 3) {
        throw StateError(bindings.takeString(update.error));
      }

      final progress = ReceiveProgress(
        uniqueBlocks: update.uniqueBlocks,
        decodedBlocks: update.decodedBlocks,
        totalBlocks: update.totalBlocks,
      );

      return switch (update.kind) {
        0 => const IgnoredQrPayload(),
        1 => ReceiveInProgress(progress),
        2 => ReceiveComplete(
          progress: progress,
          file: ReceivedFile(
            name: 'received_file',
            bytes: bindings.takeBytes(update.file),
          ),
        ),
        _ => throw StateError('unknown receive update kind ${update.kind}'),
      };
    } finally {
      calloc.free(payloadPtr);
      calloc.free(updateOut);
    }
  }

  @override
  void dispose() {
    if (_disposed) {
      return;
    }
    bindings.sessionFree(handle);
    handle = nullptr;
    _disposed = true;
  }
}

final class _NativeByteBuffer extends Struct {
  external Pointer<Uint8> ptr;

  @UintPtr()
  external int len;

  @UintPtr()
  external int cap;
}

final class _NativeReceiveUpdate extends Struct {
  @Uint32()
  external int kind;

  @Uint32()
  external int uniqueBlocks;

  @UintPtr()
  external int decodedBlocks;

  @UintPtr()
  external int totalBlocks;

  external _NativeByteBuffer file;

  external _NativeByteBuffer error;
}

final class _NativeBindings {
  _NativeBindings(DynamicLibrary library)
    : sessionNew = library.lookupFunction<_SessionNewNative, _SessionNew>(
        'transfer_mobile_session_new',
      ),
      sessionFree = library.lookupFunction<_SessionFreeNative, _SessionFree>(
        'transfer_mobile_session_free',
      ),
      sessionPublicKey = library
          .lookupFunction<_SessionPublicKeyNative, _SessionPublicKey>(
            'transfer_mobile_session_public_key',
          ),
      sessionFeedQr = library
          .lookupFunction<_SessionFeedQrNative, _SessionFeedQr>(
            'transfer_mobile_session_feed_qr',
          ),
      bufferFree = library.lookupFunction<_BufferFreeNative, _BufferFree>(
        'transfer_mobile_buffer_free',
      );

  factory _NativeBindings.open() {
    if (Platform.isAndroid) {
      return _NativeBindings(DynamicLibrary.open('libtransfer_mobile_core.so'));
    }
    throw UnsupportedError('mobile receiver native core is Android-only in v1');
  }

  final _SessionNew sessionNew;
  final _SessionFree sessionFree;
  final _SessionPublicKey sessionPublicKey;
  final _SessionFeedQr sessionFeedQr;
  final _BufferFree bufferFree;

  Uint8List takeBytes(_NativeByteBuffer buffer) {
    if (buffer.ptr == nullptr || buffer.len == 0) {
      return Uint8List(0);
    }
    try {
      return Uint8List.fromList(buffer.ptr.asTypedList(buffer.len));
    } finally {
      bufferFree(buffer);
    }
  }

  String takeString(_NativeByteBuffer buffer) {
    return utf8.decode(takeBytes(buffer));
  }
}

typedef _SessionNewNative =
    Pointer<Void> Function(Pointer<Uint8>, UintPtr, Pointer<_NativeByteBuffer>);
typedef _SessionNew =
    Pointer<Void> Function(Pointer<Uint8>, int, Pointer<_NativeByteBuffer>);

typedef _SessionFreeNative = Void Function(Pointer<Void>);
typedef _SessionFree = void Function(Pointer<Void>);

typedef _SessionPublicKeyNative = _NativeByteBuffer Function(Pointer<Void>);
typedef _SessionPublicKey = _NativeByteBuffer Function(Pointer<Void>);

typedef _SessionFeedQrNative =
    Bool Function(
      Pointer<Void>,
      Pointer<Uint8>,
      UintPtr,
      Pointer<_NativeReceiveUpdate>,
    );
typedef _SessionFeedQr =
    bool Function(
      Pointer<Void>,
      Pointer<Uint8>,
      int,
      Pointer<_NativeReceiveUpdate>,
    );

typedef _BufferFreeNative = Void Function(_NativeByteBuffer);
typedef _BufferFree = void Function(_NativeByteBuffer);

sealed class ReceiveUpdate {
  const ReceiveUpdate();
}

class IgnoredQrPayload extends ReceiveUpdate {
  const IgnoredQrPayload();
}

class ReceiveInProgress extends ReceiveUpdate {
  const ReceiveInProgress(this.progress);

  final ReceiveProgress progress;
}

class ReceiveComplete extends ReceiveUpdate {
  const ReceiveComplete({required this.progress, required this.file});

  final ReceiveProgress progress;
  final ReceivedFile file;
}

class ReceiveProgress {
  const ReceiveProgress({
    required this.uniqueBlocks,
    required this.decodedBlocks,
    required this.totalBlocks,
  });

  const ReceiveProgress.empty()
    : uniqueBlocks = 0,
      decodedBlocks = 0,
      totalBlocks = 0;

  final int uniqueBlocks;
  final int decodedBlocks;
  final int totalBlocks;
}

class ReceivedFile {
  const ReceivedFile({required this.name, required this.bytes});

  final String name;
  final Uint8List bytes;
}
