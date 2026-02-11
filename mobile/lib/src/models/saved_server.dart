import 'dart:convert';

class SavedServer {
  final String host;
  final int port;
  final String? label;
  final String? token;

  const SavedServer({
    required this.host,
    required this.port,
    this.label,
    this.token,
  });

  SavedServer copyWith({String? token}) => SavedServer(
        host: host,
        port: port,
        label: label,
        token: token ?? this.token,
      );

  String get displayName => label ?? '$host:$port';

  Map<String, dynamic> toJson() => {
        'host': host,
        'port': port,
        if (label != null) 'label': label,
        if (token != null) 'token': token,
      };

  factory SavedServer.fromJson(Map<String, dynamic> json) => SavedServer(
        host: json['host'] as String,
        port: json['port'] as int,
        label: json['label'] as String?,
        token: json['token'] as String?,
      );

  static List<SavedServer> listFromJson(String jsonString) {
    final list = jsonDecode(jsonString) as List;
    return list
        .map((e) => SavedServer.fromJson(e as Map<String, dynamic>))
        .toList();
  }

  static String listToJson(List<SavedServer> servers) =>
      jsonEncode(servers.map((s) => s.toJson()).toList());

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is SavedServer && host == other.host && port == other.port;

  @override
  int get hashCode => Object.hash(host, port);
}
