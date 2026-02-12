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
                          final newIdx = (idx + direction).clamp(0, ids.length - 1);
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
            onLongPress: () => _showCloseDialog(context, tid, name),
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

  void _showCloseDialog(
      BuildContext context, String terminalId, String name) {
    showDialog(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Close terminal'),
        content: Text('Close "$name"?'),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(ctx).pop(),
            child: const Text('Cancel'),
          ),
          TextButton(
            onPressed: () {
              Navigator.of(ctx).pop();
              state_ffi.closeTerminal(
                connId: connId,
                projectId: projectId,
                terminalId: terminalId,
              );
            },
            child: const Text('Close',
                style: TextStyle(color: Colors.redAccent)),
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
