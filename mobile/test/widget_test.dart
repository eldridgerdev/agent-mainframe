import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:shared_preferences/shared_preferences.dart';

import 'package:amf_companion/main.dart';

void main() {
  testWidgets('shows the pairing screen when no device is paired', (
    WidgetTester tester,
  ) async {
    SharedPreferences.setMockInitialValues({});

    await tester.pumpWidget(const AmfCompanionApp());
    await tester.pumpAndSettle();

    expect(find.text('Pair with AMF'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Pair'), findsOneWidget);
  });

  testWidgets('goes straight to status when a device is already paired', (
    WidgetTester tester,
  ) async {
    SharedPreferences.setMockInitialValues({
      'server_address': '127.0.0.1:1234',
      'device_id': 'dev-1',
      'token': 'test-token',
    });

    await tester.pumpWidget(const AmfCompanionApp());
    // A single pump is enough for RootScreen's async credential load to
    // resolve and switch screens — don't pumpAndSettle here, since
    // StatusScreen's loading spinner animates indefinitely until its (real,
    // network-backed) status fetch resolves, which never happens in this
    // test.
    await tester.pump();

    expect(find.text('Feature status'), findsOneWidget);
  });
}
