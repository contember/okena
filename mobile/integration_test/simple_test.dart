import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/main.dart';
import 'package:mobile/src/rust/frb_generated.dart';
import 'package:integration_test/integration_test.dart';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());
  testWidgets('App launches and shows server list', (WidgetTester tester) async {
    await tester.pumpWidget(const OkenaApp());
    await tester.pumpAndSettle();
    expect(find.text('Okena'), findsWidgets);
  });
}
