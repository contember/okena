import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/connection_provider.dart';
import '../providers/workspace_provider.dart';
import '../rust/api/state.dart' as state_ffi;
import '../widgets/project_drawer.dart';
import '../widgets/key_toolbar.dart';
import '../widgets/terminal_view.dart';

class WorkspaceScreen extends StatelessWidget {
  const WorkspaceScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final workspace = context.watch<WorkspaceProvider>();
    final connection = context.watch<ConnectionProvider>();
    final project = workspace.selectedProject;
    final connId = connection.connId;
    final selectedTerminalId = workspace.selectedTerminalId;

    return Scaffold(
      appBar: AppBar(
        title: _ProjectSwitcher(
          projects: workspace.projects,
          selectedProjectId: workspace.selectedProjectId,
          onSelect: (id) => workspace.selectProject(id),
        ),
        leading: Builder(
          builder: (ctx) => IconButton(
            icon: const Icon(Icons.menu),
            onPressed: () => Scaffold.of(ctx).openDrawer(),
          ),
        ),
        actions: [
          // Connection quality indicator
          if (connId != null)
            Padding(
              padding: const EdgeInsets.only(right: 4),
              child: _ConnectionDot(
                secondsSinceActivity: workspace.secondsSinceActivity,
              ),
            ),
          // Git status button
          if (connId != null && project != null && project.gitBranch != null)
            _GitButton(connId: connId, project: project),
          // Services button
          if (connId != null &&
              project != null &&
              project.services.isNotEmpty)
            _ServicesButton(connId: connId, project: project),
          // Fullscreen toggle
          if (connId != null && project != null && selectedTerminalId != null)
            IconButton(
              icon: Icon(
                workspace.fullscreenTerminal != null
                    ? Icons.fullscreen_exit
                    : Icons.fullscreen,
                size: 20,
              ),
              tooltip: workspace.fullscreenTerminal != null
                  ? 'Exit Fullscreen'
                  : 'Fullscreen',
              onPressed: () {
                if (workspace.fullscreenTerminal != null) {
                  state_ffi.setFullscreen(
                    connId: connId,
                    projectId: project.id,
                    terminalId: null,
                  );
                } else {
                  state_ffi.setFullscreen(
                    connId: connId,
                    projectId: project.id,
                    terminalId: selectedTerminalId,
                  );
                }
              },
            ),
          if (connId != null && project != null)
            IconButton(
              icon: const Icon(Icons.add),
              tooltip: 'New Terminal',
              onPressed: () {
                state_ffi.createTerminal(
                  connId: connId,
                  projectId: project.id,
                );
              },
            ),
        ],
      ),
      drawer: const ProjectDrawer(),
      body: connId == null || project == null
          ? const Center(child: Text('No project selected'))
          : selectedTerminalId == null
              ? Center(
                  child: Column(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      const Text(
                        'No terminals',
                        style: TextStyle(color: Colors.grey),
                      ),
                      const SizedBox(height: 16),
                      FilledButton.icon(
                        onPressed: () {
                          state_ffi.createTerminal(
                            connId: connId,
                            projectId: project.id,
                          );
                        },
                        icon: const Icon(Icons.add),
                        label: const Text('New Terminal'),
                      ),
                    ],
                  ),
                )
              : Column(
                  children: [
                    if (project.terminalIds.length > 1)
                      _TerminalTabBar(
                        terminalIds: project.terminalIds,
                        terminalNames: project.terminalNames,
                        selectedTerminalId: selectedTerminalId,
                        projectId: project.id,
                        connId: connId,
                        onSelect: (id) => workspace.selectTerminal(id),
                      ),
                    Expanded(
                      child: TerminalView(
                        connId: connId,
                        terminalId: selectedTerminalId,
                        onTerminalSwipe: (direction) {
                          final ids = project.terminalIds;
                          if (ids.length <= 1) return;
                          final idx = ids.indexOf(selectedTerminalId);
                          if (idx < 0) return;
                          final newIdx =
                              (idx + direction).clamp(0, ids.length - 1);
                          if (newIdx != idx) {
                            workspace.selectTerminal(ids[newIdx]);
                          }
                        },
                      ),
                    ),
                    KeyToolbar(
                      connId: connId,
                      terminalId: selectedTerminalId,
                    ),
                  ],
                ),
    );
  }
}

/// Tappable project name in AppBar that opens a dropdown to switch projects.
class _ProjectSwitcher extends StatelessWidget {
  final List<state_ffi.ProjectInfo> projects;
  final String? selectedProjectId;
  final ValueChanged<String> onSelect;

  const _ProjectSwitcher({
    required this.projects,
    required this.selectedProjectId,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    final selected = projects
            .where((p) => p.id == selectedProjectId)
            .firstOrNull ??
        projects.firstOrNull;
    final name = selected?.name ?? 'No Project';

    if (projects.length <= 1) {
      return Text(name);
    }

    return GestureDetector(
      onTap: () => _showProjectMenu(context),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Flexible(
            child: Text(
              name,
              overflow: TextOverflow.ellipsis,
            ),
          ),
          const SizedBox(width: 4),
          const Icon(Icons.arrow_drop_down, size: 20),
        ],
      ),
    );
  }

  void _showProjectMenu(BuildContext context) {
    final RenderBox button = context.findRenderObject() as RenderBox;
    final overlay =
        Overlay.of(context).context.findRenderObject() as RenderBox;
    final position = RelativeRect.fromRect(
      Rect.fromPoints(
        button.localToGlobal(Offset(0, button.size.height), ancestor: overlay),
        button.localToGlobal(button.size.bottomRight(Offset.zero),
            ancestor: overlay),
      ),
      Offset.zero & overlay.size,
    );

    showMenu<String>(
      context: context,
      position: position,
      items: projects.map((p) {
        return PopupMenuItem<String>(
          value: p.id,
          child: Row(
            children: [
              Icon(
                Icons.folder,
                size: 18,
                color: p.id == selectedProjectId
                    ? Theme.of(context).colorScheme.primary
                    : null,
              ),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  p.name,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(
                    fontWeight: p.id == selectedProjectId
                        ? FontWeight.bold
                        : FontWeight.normal,
                  ),
                ),
              ),
              if (p.gitBranch != null) ...[
                const SizedBox(width: 8),
                Icon(Icons.commit, size: 14, color: Colors.grey[500]),
                const SizedBox(width: 2),
                Text(
                  p.gitBranch!,
                  style: TextStyle(fontSize: 11, color: Colors.grey[500]),
                ),
              ],
            ],
          ),
        );
      }).toList(),
    ).then((value) {
      if (value != null) {
        onSelect(value);
      }
    });
  }
}

/// Horizontal tab bar showing terminals in the current project.
class _TerminalTabBar extends StatelessWidget {
  final List<String> terminalIds;
  final Map<String, String> terminalNames;
  final String selectedTerminalId;
  final String projectId;
  final String connId;
  final ValueChanged<String> onSelect;

  const _TerminalTabBar({
    required this.terminalIds,
    required this.terminalNames,
    required this.selectedTerminalId,
    required this.projectId,
    required this.connId,
    required this.onSelect,
  });

  @override
  Widget build(BuildContext context) {
    return Container(
      height: 36,
      color: const Color(0xFF252526),
      child: ListView.builder(
        scrollDirection: Axis.horizontal,
        itemCount: terminalIds.length,
        padding: const EdgeInsets.symmetric(horizontal: 4),
        itemBuilder: (context, index) {
          final tid = terminalIds[index];
          final isSelected = tid == selectedTerminalId;
          final name = terminalNames[tid] ?? 'Terminal ${index + 1}';

          return GestureDetector(
            onTap: () => onSelect(tid),
            onLongPress: () => _showTabMenu(context, tid, name),
            child: Container(
              margin: const EdgeInsets.symmetric(horizontal: 2, vertical: 4),
              padding: const EdgeInsets.symmetric(horizontal: 12),
              decoration: BoxDecoration(
                color: isSelected
                    ? const Color(0xFF3C3C3C)
                    : Colors.transparent,
                borderRadius: BorderRadius.circular(4),
              ),
              alignment: Alignment.center,
              child: Text(
                name,
                style: TextStyle(
                  color: isSelected ? Colors.white : Colors.white54,
                  fontSize: 12,
                  fontFamily: 'JetBrainsMono',
                  fontWeight:
                      isSelected ? FontWeight.bold : FontWeight.normal,
                ),
              ),
            ),
          );
        },
      ),
    );
  }

  void _showTabMenu(BuildContext context, String terminalId, String name) {
    showModalBottomSheet(
      context: context,
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Padding(
              padding: const EdgeInsets.all(16),
              child: Text(name,
                  style: Theme.of(context).textTheme.titleMedium),
            ),
            ListTile(
              leading: const Icon(Icons.edit),
              title: const Text('Rename'),
              onTap: () {
                Navigator.of(ctx).pop();
                _showRenameDialog(context, terminalId, name);
              },
            ),
            ListTile(
              leading: const Icon(Icons.close, color: Colors.redAccent),
              title: const Text('Close',
                  style: TextStyle(color: Colors.redAccent)),
              onTap: () {
                Navigator.of(ctx).pop();
                state_ffi.closeTerminal(
                  connId: connId,
                  projectId: projectId,
                  terminalId: terminalId,
                );
              },
            ),
          ],
        ),
      ),
    );
  }

  void _showRenameDialog(
      BuildContext context, String terminalId, String currentName) {
    final controller = TextEditingController(text: currentName);
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Rename Terminal'),
        content: TextField(
          controller: controller,
          autofocus: true,
          decoration: const InputDecoration(labelText: 'Name'),
          onSubmitted: (_) {
            Navigator.of(ctx).pop();
            state_ffi.renameTerminal(
              connId: connId,
              projectId: projectId,
              terminalId: terminalId,
              name: controller.text,
            );
          },
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () {
              Navigator.of(ctx).pop();
              state_ffi.renameTerminal(
                connId: connId,
                projectId: projectId,
                terminalId: terminalId,
                name: controller.text,
              );
            },
            child: const Text('Rename'),
          ),
        ],
      ),
    );
  }
}

/// Small colored dot indicating connection quality.
class _ConnectionDot extends StatelessWidget {
  final double secondsSinceActivity;

  const _ConnectionDot({required this.secondsSinceActivity});

  @override
  Widget build(BuildContext context) {
    final Color color;
    if (secondsSinceActivity < 3) {
      color = Colors.green;
    } else if (secondsSinceActivity < 10) {
      color = Colors.orange;
    } else {
      color = Colors.red;
    }

    return Container(
      width: 8,
      height: 8,
      decoration: BoxDecoration(
        color: color,
        shape: BoxShape.circle,
      ),
    );
  }
}

/// Git status button in the app bar.
class _GitButton extends StatelessWidget {
  final String connId;
  final state_ffi.ProjectInfo project;

  const _GitButton({required this.connId, required this.project});

  @override
  Widget build(BuildContext context) {
    final hasChanges = project.gitLinesAdded > 0 || project.gitLinesRemoved > 0;

    return IconButton(
      icon: Badge(
        isLabelVisible: hasChanges,
        smallSize: 8,
        child: const Icon(Icons.commit, size: 20),
      ),
      tooltip: project.gitBranch ?? 'Git',
      onPressed: () => _showGitSheet(context),
    );
  }

  void _showGitSheet(BuildContext context) {
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      builder: (ctx) => DraggableScrollableSheet(
        initialChildSize: 0.6,
        minChildSize: 0.3,
        maxChildSize: 0.9,
        expand: false,
        builder: (ctx, scrollController) => _GitSheet(
          connId: connId,
          project: project,
          scrollController: scrollController,
        ),
      ),
    );
  }
}

class _GitSheet extends StatefulWidget {
  final String connId;
  final state_ffi.ProjectInfo project;
  final ScrollController scrollController;

  const _GitSheet({
    required this.connId,
    required this.project,
    required this.scrollController,
  });

  @override
  State<_GitSheet> createState() => _GitSheetState();
}

class _GitSheetState extends State<_GitSheet> {
  String? _diffSummary;
  String? _branches;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    _loadData();
  }

  Future<void> _loadData() async {
    try {
      final results = await Future.wait([
        state_ffi.gitDiffSummary(
            connId: widget.connId, projectId: widget.project.id),
        state_ffi.gitBranches(
            connId: widget.connId, projectId: widget.project.id),
      ]);
      if (mounted) {
        setState(() {
          _diffSummary = results[0];
          _branches = results[1];
          _loading = false;
        });
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          _loading = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        // Handle bar
        Container(
          margin: const EdgeInsets.only(top: 12, bottom: 8),
          width: 32,
          height: 4,
          decoration: BoxDecoration(
            color: Colors.grey[600],
            borderRadius: BorderRadius.circular(2),
          ),
        ),
        // Header
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          child: Row(
            children: [
              const Icon(Icons.commit, size: 20),
              const SizedBox(width: 8),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      widget.project.gitBranch ?? 'Unknown branch',
                      style: Theme.of(context).textTheme.titleMedium,
                    ),
                    Row(
                      children: [
                        if (widget.project.gitLinesAdded > 0)
                          Text(
                            '+${widget.project.gitLinesAdded}',
                            style: TextStyle(
                                fontSize: 12, color: Colors.green[400]),
                          ),
                        if (widget.project.gitLinesAdded > 0 &&
                            widget.project.gitLinesRemoved > 0)
                          const SizedBox(width: 8),
                        if (widget.project.gitLinesRemoved > 0)
                          Text(
                            '-${widget.project.gitLinesRemoved}',
                            style: TextStyle(
                                fontSize: 12, color: Colors.red[400]),
                          ),
                      ],
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
        const Divider(),
        Expanded(
          child: _loading
              ? const Center(child: CircularProgressIndicator())
              : ListView(
                  controller: widget.scrollController,
                  padding: const EdgeInsets.all(16),
                  children: [
                    if (_diffSummary != null) ...[
                      Text('Changes',
                          style: Theme.of(context).textTheme.titleSmall),
                      const SizedBox(height: 8),
                      _DiffSummaryView(json: _diffSummary!),
                      const SizedBox(height: 16),
                    ],
                    if (_branches != null) ...[
                      Text('Branches',
                          style: Theme.of(context).textTheme.titleSmall),
                      const SizedBox(height: 8),
                      _BranchesView(json: _branches!),
                    ],
                  ],
                ),
        ),
      ],
    );
  }
}

class _DiffSummaryView extends StatelessWidget {
  final String json;

  const _DiffSummaryView({required this.json});

  @override
  Widget build(BuildContext context) {
    try {
      final data = jsonDecode(json);
      if (data is Map && data.containsKey('files')) {
        final files = data['files'] as List? ?? [];
        if (files.isEmpty) {
          return Text('No changes',
              style: TextStyle(color: Colors.grey[500]));
        }
        return Column(
          children: files.map<Widget>((f) {
            final file = f as Map<String, dynamic>;
            final path = file['path'] as String? ?? '';
            final added = file['added'] as int? ?? 0;
            final removed = file['removed'] as int? ?? 0;
            final status = file['status'] as String? ?? 'modified';

            IconData icon;
            Color iconColor;
            switch (status) {
              case 'added':
                icon = Icons.add_circle_outline;
                iconColor = Colors.green;
                break;
              case 'deleted':
                icon = Icons.remove_circle_outline;
                iconColor = Colors.red;
                break;
              default:
                icon = Icons.edit;
                iconColor = Colors.orange;
            }

            return Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Row(
                children: [
                  Icon(icon, size: 16, color: iconColor),
                  const SizedBox(width: 8),
                  Expanded(
                    child: Text(
                      path,
                      style: const TextStyle(
                          fontSize: 12, fontFamily: 'JetBrainsMono'),
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                  if (added > 0)
                    Text('+$added',
                        style: TextStyle(
                            fontSize: 11, color: Colors.green[400])),
                  if (added > 0 && removed > 0) const SizedBox(width: 4),
                  if (removed > 0)
                    Text('-$removed',
                        style:
                            TextStyle(fontSize: 11, color: Colors.red[400])),
                ],
              ),
            );
          }).toList(),
        );
      }
      // Fallback: show as plain text
      return Text(json,
          style: const TextStyle(fontSize: 12, fontFamily: 'JetBrainsMono'));
    } catch (_) {
      return Text(json,
          style: const TextStyle(fontSize: 12, fontFamily: 'JetBrainsMono'));
    }
  }
}

class _BranchesView extends StatelessWidget {
  final String json;

  const _BranchesView({required this.json});

  @override
  Widget build(BuildContext context) {
    try {
      final data = jsonDecode(json);
      if (data is Map && data.containsKey('branches')) {
        final branches = data['branches'] as List? ?? [];
        return Column(
          children: branches.map<Widget>((b) {
            final branch = b as Map<String, dynamic>;
            final name = branch['name'] as String? ?? '';
            final isCurrent = branch['current'] as bool? ?? false;
            final isRemote = branch['remote'] as bool? ?? false;

            return Padding(
              padding: const EdgeInsets.symmetric(vertical: 2),
              child: Row(
                children: [
                  Icon(
                    isCurrent ? Icons.check_circle : Icons.circle_outlined,
                    size: 16,
                    color: isCurrent ? Colors.green : Colors.grey[600],
                  ),
                  const SizedBox(width: 8),
                  if (isRemote)
                    Padding(
                      padding: const EdgeInsets.only(right: 4),
                      child: Icon(Icons.cloud,
                          size: 12, color: Colors.grey[500]),
                    ),
                  Expanded(
                    child: Text(
                      name,
                      style: TextStyle(
                        fontSize: 12,
                        fontFamily: 'JetBrainsMono',
                        fontWeight:
                            isCurrent ? FontWeight.bold : FontWeight.normal,
                      ),
                      overflow: TextOverflow.ellipsis,
                    ),
                  ),
                ],
              ),
            );
          }).toList(),
        );
      }
      return Text(json,
          style: const TextStyle(fontSize: 12, fontFamily: 'JetBrainsMono'));
    } catch (_) {
      return Text(json,
          style: const TextStyle(fontSize: 12, fontFamily: 'JetBrainsMono'));
    }
  }
}

/// Services button in the app bar.
class _ServicesButton extends StatelessWidget {
  final String connId;
  final state_ffi.ProjectInfo project;

  const _ServicesButton({required this.connId, required this.project});

  @override
  Widget build(BuildContext context) {
    final running =
        project.services.where((s) => s.status == 'running').length;
    final total = project.services.length;

    return IconButton(
      icon: Badge(
        isLabelVisible: running > 0,
        label: Text('$running',
            style: const TextStyle(fontSize: 9)),
        child: const Icon(Icons.dns, size: 20),
      ),
      tooltip: '$running/$total services running',
      onPressed: () => _showServicesSheet(context),
    );
  }

  void _showServicesSheet(BuildContext context) {
    showModalBottomSheet(
      context: context,
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              margin: const EdgeInsets.only(top: 12, bottom: 8),
              width: 32,
              height: 4,
              decoration: BoxDecoration(
                color: Colors.grey[600],
                borderRadius: BorderRadius.circular(2),
              ),
            ),
            Padding(
              padding:
                  const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
              child: Row(
                children: [
                  const Icon(Icons.dns, size: 20),
                  const SizedBox(width: 8),
                  Text('Services',
                      style: Theme.of(context).textTheme.titleMedium),
                  const Spacer(),
                  TextButton(
                    onPressed: () {
                      state_ffi.startAllServices(
                        connId: connId,
                        projectId: project.id,
                      );
                    },
                    child: const Text('Start All'),
                  ),
                  TextButton(
                    onPressed: () {
                      state_ffi.stopAllServices(
                        connId: connId,
                        projectId: project.id,
                      );
                    },
                    child: const Text('Stop All',
                        style: TextStyle(color: Colors.redAccent)),
                  ),
                ],
              ),
            ),
            const Divider(),
            ...project.services.map((s) => _ServiceTile(
                  service: s,
                  connId: connId,
                  projectId: project.id,
                )),
            const SizedBox(height: 16),
          ],
        ),
      ),
    );
  }
}

class _ServiceTile extends StatelessWidget {
  final state_ffi.ServiceInfo service;
  final String connId;
  final String projectId;

  const _ServiceTile({
    required this.service,
    required this.connId,
    required this.projectId,
  });

  Color _statusColor() {
    switch (service.status) {
      case 'running':
        return Colors.green;
      case 'stopped':
        return Colors.grey;
      case 'crashed':
        return Colors.red;
      case 'starting':
      case 'restarting':
        return Colors.orange;
      default:
        return Colors.grey;
    }
  }

  @override
  Widget build(BuildContext context) {
    final color = _statusColor();
    return ListTile(
      leading: Container(
        width: 8,
        height: 8,
        decoration: BoxDecoration(color: color, shape: BoxShape.circle),
      ),
      title: Row(
        children: [
          Expanded(child: Text(service.name)),
          Text(
            service.status,
            style: TextStyle(fontSize: 12, color: color),
          ),
        ],
      ),
      subtitle: service.ports.isNotEmpty
          ? Text(
              service.ports.map((p) => ':$p').join(', '),
              style: TextStyle(fontSize: 11, color: Colors.grey[500]),
            )
          : null,
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (service.status == 'running') ...[
            IconButton(
              icon: const Icon(Icons.restart_alt, size: 20),
              tooltip: 'Restart',
              onPressed: () {
                state_ffi.restartService(
                  connId: connId,
                  projectId: projectId,
                  serviceName: service.name,
                );
              },
            ),
            IconButton(
              icon: Icon(Icons.stop, size: 20, color: Colors.red[300]),
              tooltip: 'Stop',
              onPressed: () {
                state_ffi.stopService(
                  connId: connId,
                  projectId: projectId,
                  serviceName: service.name,
                );
              },
            ),
          ] else ...[
            IconButton(
              icon: Icon(Icons.play_arrow, size: 20, color: Colors.green[300]),
              tooltip: 'Start',
              onPressed: () {
                state_ffi.startService(
                  connId: connId,
                  projectId: projectId,
                  serviceName: service.name,
                );
              },
            ),
          ],
        ],
      ),
    );
  }
}
