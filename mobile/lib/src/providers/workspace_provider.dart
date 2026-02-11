import 'dart:async';

import 'package:flutter/foundation.dart';

import 'connection_provider.dart';
import '../../src/rust/api/state.dart' as ffi;

class WorkspaceProvider extends ChangeNotifier {
  final ConnectionProvider _connection;
  List<ffi.ProjectInfo> _projects = [];
  String? _selectedProjectId;
  String? _selectedTerminalId;
  Timer? _pollTimer;

  List<ffi.ProjectInfo> get projects => _projects;
  String? get selectedProjectId => _selectedProjectId;
  String? get selectedTerminalId => _selectedTerminalId;

  ffi.ProjectInfo? get selectedProject {
    if (_selectedProjectId == null) return _projects.firstOrNull;
    return _projects
        .where((p) => p.id == _selectedProjectId)
        .firstOrNull ?? _projects.firstOrNull;
  }

  WorkspaceProvider(this._connection) {
    _connection.addListener(_onConnectionChanged);
    if (_connection.isConnected) {
      _startPolling();
    }
  }

  void selectProject(String projectId) {
    _selectedProjectId = projectId;
    _selectedTerminalId = null;
    notifyListeners();
  }

  void selectTerminal(String terminalId) {
    _selectedTerminalId = terminalId;
    notifyListeners();
  }

  void _onConnectionChanged() {
    if (_connection.isConnected) {
      _startPolling();
    } else {
      _stopPolling();
      _projects = [];
      _selectedProjectId = null;
      _selectedTerminalId = null;
      notifyListeners();
    }
  }

  void _startPolling() {
    _pollTimer?.cancel();
    _pollTimer = Timer.periodic(
      const Duration(seconds: 1),
      (_) => _pollState(),
    );
    // Immediate first poll
    _pollState();
  }

  void _stopPolling() {
    _pollTimer?.cancel();
    _pollTimer = null;
  }

  void _pollState() {
    final connId = _connection.connId;
    if (connId == null) return;

    final newProjects = ffi.getProjects(connId: connId);
    final focusedId = ffi.getFocusedProjectId(connId: connId);

    bool changed = false;

    if (!_projectListEquals(newProjects, _projects)) {
      _projects = newProjects;
      changed = true;
    }

    // Auto-select the focused project if we don't have a selection
    if (_selectedProjectId == null && focusedId != null) {
      _selectedProjectId = focusedId;
      changed = true;
    }

    // Auto-select first terminal if none selected or current selection is gone
    final project = selectedProject;
    if (project != null && project.terminalIds.isNotEmpty) {
      if (_selectedTerminalId == null ||
          !project.terminalIds.contains(_selectedTerminalId)) {
        _selectedTerminalId = project.terminalIds.first;
        changed = true;
      }
    } else if (_selectedTerminalId != null) {
      _selectedTerminalId = null;
      changed = true;
    }

    if (changed) {
      notifyListeners();
    }
  }

  bool _projectListEquals(
      List<ffi.ProjectInfo> a, List<ffi.ProjectInfo> b) {
    if (a.length != b.length) return false;
    for (int i = 0; i < a.length; i++) {
      if (a[i].id != b[i].id || a[i].name != b[i].name) return false;
      if (!listEquals(a[i].terminalIds, b[i].terminalIds)) return false;
    }
    return true;
  }

  @override
  void dispose() {
    _connection.removeListener(_onConnectionChanged);
    _stopPolling();
    super.dispose();
  }
}
