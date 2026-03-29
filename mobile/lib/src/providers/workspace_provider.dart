import 'dart:async';

import 'package:flutter/foundation.dart';

import 'connection_provider.dart';
import '../../src/rust/api/state.dart' as ffi;
import '../../src/rust/api/connection.dart' as conn_ffi;

class WorkspaceProvider extends ChangeNotifier {
  final ConnectionProvider _connection;
  List<ffi.ProjectInfo> _projects = [];
  List<ffi.FolderInfo> _folders = [];
  List<String> _projectOrder = [];
  ffi.FullscreenInfo? _fullscreenTerminal;
  String? _selectedProjectId;
  String? _selectedTerminalId;
  Set<String>? _previousTerminalIds;
  Timer? _pollTimer;
  double _secondsSinceActivity = 0;

  List<ffi.ProjectInfo> get projects => _projects;
  List<ffi.FolderInfo> get folders => _folders;
  List<String> get projectOrder => _projectOrder;
  ffi.FullscreenInfo? get fullscreenTerminal => _fullscreenTerminal;
  String? get selectedProjectId => _selectedProjectId;
  String? get selectedTerminalId => _selectedTerminalId;
  double get secondsSinceActivity => _secondsSinceActivity;

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
    _previousTerminalIds = null;
    notifyListeners();
  }

  void selectTerminal(String terminalId) {
    _selectedTerminalId = terminalId;
    notifyListeners();
  }

  /// Get the layout JSON for the selected project.
  String? getProjectLayoutJson() {
    final connId = _connection.connId;
    final projectId = _selectedProjectId ?? selectedProject?.id;
    if (connId == null || projectId == null) return null;
    return ffi.getProjectLayoutJson(connId: connId, projectId: projectId);
  }

  void _onConnectionChanged() {
    if (_connection.isConnected) {
      _startPolling();
    } else {
      _stopPolling();
      _projects = [];
      _folders = [];
      _projectOrder = [];
      _fullscreenTerminal = null;
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
    final newFolders = ffi.getFolders(connId: connId);
    final newProjectOrder = ffi.getProjectOrder(connId: connId);
    final newFullscreen = ffi.getFullscreenTerminal(connId: connId);

    bool changed = false;

    if (!_projectListEquals(newProjects, _projects)) {
      _projects = newProjects;
      changed = true;
    }

    if (!listEquals(newFolders.map((f) => f.id).toList(),
        _folders.map((f) => f.id).toList())) {
      _folders = newFolders;
      changed = true;
    }

    if (!listEquals(newProjectOrder, _projectOrder)) {
      _projectOrder = newProjectOrder;
      changed = true;
    }

    if (newFullscreen?.terminalId != _fullscreenTerminal?.terminalId) {
      _fullscreenTerminal = newFullscreen;
      changed = true;
    }

    // Auto-select the focused project if we don't have a selection
    if (_selectedProjectId == null && focusedId != null) {
      _selectedProjectId = focusedId;
      changed = true;
    }

    // Auto-select terminal: pick newly added terminal, or first if current gone
    final project = selectedProject;
    if (project != null && project.terminalIds.isNotEmpty) {
      if (_selectedTerminalId == null ||
          !project.terminalIds.contains(_selectedTerminalId)) {
        _selectedTerminalId = project.terminalIds.first;
        changed = true;
      } else if (_previousTerminalIds != null) {
        // Find newly added terminals
        final newIds = project.terminalIds
            .where((id) => !_previousTerminalIds!.contains(id))
            .toList();
        if (newIds.isNotEmpty) {
          _selectedTerminalId = newIds.last;
          changed = true;
        }
      }
      _previousTerminalIds = Set.of(project.terminalIds);
    } else {
      _previousTerminalIds = null;
      if (_selectedTerminalId != null) {
        _selectedTerminalId = null;
        changed = true;
      }
    }

    // Poll connection health
    final newActivity = conn_ffi.secondsSinceActivity(connId: connId);
    if ((_secondsSinceActivity < 3) != (newActivity < 3) ||
        (_secondsSinceActivity < 10) != (newActivity < 10)) {
      changed = true;
    }
    _secondsSinceActivity = newActivity;

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
      // Check git status changes
      if (a[i].gitBranch != b[i].gitBranch) return false;
      if (a[i].gitLinesAdded != b[i].gitLinesAdded) return false;
      if (a[i].gitLinesRemoved != b[i].gitLinesRemoved) return false;
      // Check services changes
      if (a[i].services.length != b[i].services.length) return false;
      for (int j = 0; j < a[i].services.length; j++) {
        if (a[i].services[j].name != b[i].services[j].name ||
            a[i].services[j].status != b[i].services[j].status) {
          return false;
        }
      }
      if (a[i].folderColor != b[i].folderColor) return false;
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
