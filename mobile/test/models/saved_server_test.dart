import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/src/models/saved_server.dart';

void main() {
  group('SavedServer', () {
    test('toJson/fromJson round-trip without label', () {
      const server = SavedServer(host: '192.168.1.100', port: 19100);
      final json = server.toJson();
      final restored = SavedServer.fromJson(json);

      expect(restored.host, '192.168.1.100');
      expect(restored.port, 19100);
      expect(restored.label, isNull);
    });

    test('toJson/fromJson round-trip with label', () {
      const server =
          SavedServer(host: '10.0.0.1', port: 19200, label: 'Home PC');
      final json = server.toJson();
      final restored = SavedServer.fromJson(json);

      expect(restored.host, '10.0.0.1');
      expect(restored.port, 19200);
      expect(restored.label, 'Home PC');
    });

    test('label omitted from JSON when null', () {
      const server = SavedServer(host: 'host', port: 1234);
      final json = server.toJson();
      expect(json.containsKey('label'), isFalse);
    });

    test('listToJson/listFromJson round-trip', () {
      const servers = [
        SavedServer(host: 'a.com', port: 100),
        SavedServer(host: 'b.com', port: 200, label: 'B'),
      ];
      final jsonStr = SavedServer.listToJson(servers);
      final restored = SavedServer.listFromJson(jsonStr);

      expect(restored.length, 2);
      expect(restored[0].host, 'a.com');
      expect(restored[1].label, 'B');
    });

    test('displayName uses label when present', () {
      const server = SavedServer(host: 'h', port: 1, label: 'My Server');
      expect(server.displayName, 'My Server');
    });

    test('displayName falls back to host:port', () {
      const server = SavedServer(host: '10.0.0.1', port: 19100);
      expect(server.displayName, '10.0.0.1:19100');
    });

    test('equality by host and port', () {
      const a = SavedServer(host: 'x', port: 1);
      const b = SavedServer(host: 'x', port: 1, label: 'different');
      const c = SavedServer(host: 'y', port: 1);

      expect(a, equals(b));
      expect(a, isNot(equals(c)));
    });
  });
}
