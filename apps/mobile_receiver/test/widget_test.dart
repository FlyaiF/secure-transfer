import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile_receiver/main.dart';

void main() {
  testWidgets('shows private key entry first', (WidgetTester tester) async {
    await tester.pumpWidget(const ReceiverApp());

    expect(find.text('Receiver key'), findsOneWidget);
    expect(find.text('Private key'), findsOneWidget);
    expect(find.byIcon(Icons.qr_code_scanner), findsOneWidget);
  });

  testWidgets('validates private key length before starting', (
    WidgetTester tester,
  ) async {
    await tester.pumpWidget(const ReceiverApp());

    await tester.enterText(find.byType(TextField), 'short');
    await tester.tap(find.text('Start scanning'));
    await tester.pump();

    expect(
      find.text('Private key must be base64 text for exactly 32 bytes.'),
      findsOneWidget,
    );
  });

  testWidgets('starts scanner when core accepts private key', (
    WidgetTester tester,
  ) async {
    final core = _FakeReceiverCore();
    await tester.pumpWidget(ReceiverApp(core: core));

    await tester.enterText(
      find.byType(TextField),
      'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=',
    );
    await tester.tap(find.text('Start scanning'));
    await tester.pump();

    expect(core.createdSessions, 1);
    expect(find.text('Waiting for first frame'), findsOneWidget);
  });
}

class _FakeReceiverCore implements ReceiverCore {
  int createdSessions = 0;

  @override
  Future<ReceiverSession> createSession(String privateKeyBase64) async {
    createdSessions += 1;
    return _FakeReceiverSession();
  }
}

class _FakeReceiverSession implements ReceiverSession {
  @override
  String get publicKeyBase64 => 'public-key';

  @override
  Future<ReceiveUpdate> feedQrPayload(Uint8List payload) async {
    return const IgnoredQrPayload();
  }

  @override
  void dispose() {}
}
