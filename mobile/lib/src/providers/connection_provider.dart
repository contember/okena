import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:shared_preferences/shared_preferences.dart';

import '../models/saved_server.dart';
import '../../src/rust/api/connection.dart' as ffi;

const _kSavedServersKey = 'saved_servers';

class ConnectionProvider extends ChangeNotifier {
  List<SavedServer> _servers = [];
  SavedServer? _activeServer;
  String? _connId;
  ffi.ConnectionStatus _status = const ffi.ConnectionStatus.disconnected();
  Timer? _pollTimer;

  List<SavedServer> get servers => _servers;
  SavedServer? get activeServer => _activeServer;
  String? get connId => _connId;
  ffi.ConnectionStatus get status => _status;

  bool get isConnected => _status is ffi.ConnectionStatus_Connected;
  bool get isPairing => _status is ffi.ConnectionStatus_Pairing;
  bool get isConnecting => _status is ffi.ConnectionStatus_Connecting;
  bool get isDisconnected =>
      _status is ffi.ConnectionStatus_Disconnected && _activeServer == null;

  ConnectionProvider() {
    _loadServers();
  }

  Future<void> _loadServers() async {
    final prefs = await SharedPreferences.getInstance();
    final json = prefs.getString(_kSavedServersKey);
    if (json != null) {
      try {
        _servers = SavedServer.listFromJson(json);
        notifyListeners();
      } catch (_) {
        // Corrupted data — start fresh
      }
    }
  }

  Future<void> _saveServers() async {
    final prefs = await SharedPreferences.getInstance();
    await prefs.setString(_kSavedServersKey, SavedServer.listToJson(_servers));
  }

  void addServer(SavedServer server) {
    if (!_servers.contains(server)) {
      _servers.add(server);
      _saveServers();
      notifyListeners();
    }
  }

  void removeServer(SavedServer server) {
    _servers.remove(server);
    _saveServers();
    notifyListeners();
  }

  void connectTo(SavedServer server) {
    // Disconnect existing connection first
    if (_connId != null) {
      ffi.disconnect(connId: _connId!);
      _stopPolling();
    }

    _activeServer = server;
    _connId = ffi.connect(host: server.host, port: server.port);
    _status = const ffi.ConnectionStatus.connecting();
    notifyListeners();
    _startPolling(fast: true);
  }

  Future<void> pair(String code) async {
    if (_connId == null) return;
    try {
      await ffi.pair(connId: _connId!, code: code);
    } catch (e) {
      _status = ffi.ConnectionStatus.error(message: e.toString());
      notifyListeners();
    }
  }

  void disconnect() {
    if (_connId != null) {
      ffi.disconnect(connId: _connId!);
    }
    _stopPolling();
    _connId = null;
    _activeServer = null;
    _status = const ffi.ConnectionStatus.disconnected();
    notifyListeners();
  }

  void _startPolling({bool fast = false}) {
    _pollTimer?.cancel();
    _pollTimer = Timer.periodic(
      Duration(milliseconds: fast ? 500 : 2000),
      (_) => _pollStatus(),
    );
  }

  void _stopPolling() {
    _pollTimer?.cancel();
    _pollTimer = null;
  }

  void _pollStatus() {
    if (_connId == null) return;
    final oldStatus = _status;
    _status = ffi.connectionStatus(connId: _connId!);

    // Switch to slow polling once connected
    if (_status is ffi.ConnectionStatus_Connected &&
        oldStatus is! ffi.ConnectionStatus_Connected) {
      _stopPolling();
      _startPolling(fast: false);
    }

    // Stop polling on disconnect or error
    if (_status is ffi.ConnectionStatus_Disconnected ||
        _status is ffi.ConnectionStatus_Error) {
      _stopPolling();
    }

    if (_status != oldStatus) {
      notifyListeners();
    }
  }

  @override
  void dispose() {
    _stopPolling();
    if (_connId != null) {
      ffi.disconnect(connId: _connId!);
    }
    super.dispose();
  }
}
