import 'dart:convert';

import 'package:flutter_test/flutter_test.dart';
import 'package:flutter_hbb/common.dart';

void main() {
  test('shared server config preserves ASCII and Unicode fields', () {
    for (final host in ['server.example.com', '家人.example.com', '家庭😀']) {
      final config = ServerConfig(
        idServer: host,
        relayServer: '中继.example.com',
        apiServer: 'https://example.com/家庭',
        key: 'AbCdR1c1E=',
      );
      final encoded = config.encode();
      final bytes = base64Url.decode(encoded.split('').reversed.join());
      final json = jsonDecode(utf8.decode(bytes)) as Map<String, dynamic>;
      expect(json['host'], config.idServer);
      expect(json['relay'], config.relayServer);
      expect(json['api'], config.apiServer);
      expect(json['key'], config.key);

      final decoded = ServerConfig.decode(encoded);
      expect(decoded.idServer, config.idServer);
      expect(decoded.relayServer, config.relayServer);
      expect(decoded.apiServer, config.apiServer);
      expect(decoded.key, config.key);
    }
  });

  test('shared ASCII config keeps the existing wire format', () {
    final config = ServerConfig(
      idServer: 'server.example.com',
      relayServer: 'relay.example.com',
      apiServer: 'https://api.example.com',
      key: 'AbCdR1c1E=',
    );
    final legacyJson = jsonEncode({
      'host': config.idServer,
      'relay': config.relayServer,
      'api': config.apiServer,
      'key': config.key,
    });
    final legacyEncoded = base64Url.encode(legacyJson.codeUnits)
        .split('').reversed.join();
    expect(config.encode(), legacyEncoded);
  });

  test('server config still accepts legacy plain JSON', () {
    final config = ServerConfig.decode('{"host":"家人.example.com"}');
    expect(config.idServer, '家人.example.com');
    expect(config.relayServer, isEmpty);
    expect(config.apiServer, isEmpty);
    expect(config.key, isEmpty);
  });

  test('server config rejects malformed UTF-8 payloads', () {
    final malformed = base64UrlEncode(<int>[0xff, 0xfe])
        .split('').reversed.join();
    expect(() => ServerConfig.decode(malformed), throwsFormatException);
  });

  test('server config rejects non-string fields', () {
    final json = jsonEncode({'host': 123});
    expect(() => ServerConfig.decode(json), throwsFormatException);
  });
}
