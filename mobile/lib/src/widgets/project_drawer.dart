import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/connection_provider.dart';
import '../providers/workspace_provider.dart';
import '../rust/api/state.dart' as state_ffi;
import 'status_indicator.dart';

class ProjectDrawer extends StatelessWidget {
  const ProjectDrawer({super.key});

  @override
  Widget build(BuildContext context) {
    final workspace = context.watch<WorkspaceProvider>();
    final connection = context.watch<ConnectionProvider>();

    return Drawer(
      child: Column(
        children: [
          DrawerHeader(
            decoration: BoxDecoration(
              color: Theme.of(context).colorScheme.surfaceContainerHighest,
            ),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(
                  'Okena',
                  style: Theme.of(context).textTheme.headlineSmall,
                ),
                const SizedBox(height: 4),
                if (connection.activeServer != null)
                  Text(
                    connection.activeServer!.displayName,
                    style: Theme.of(context).textTheme.bodySmall,
                  ),
                const Spacer(),
                StatusIndicator(status: connection.status),
              ],
            ),
          ),
          Expanded(
            child: _ProjectList(
              workspace: workspace,
              connection: connection,
            ),
          ),
          const Divider(height: 1),
          ListTile(
            leading: const Icon(Icons.link_off),
            title: const Text('Disconnect'),
            onTap: () {
              Navigator.of(context).pop();
              connection.disconnect();
            },
          ),
        ],
      ),
    );
  }
}

class _ProjectList extends StatelessWidget {
  final WorkspaceProvider workspace;
  final ConnectionProvider connection;

  const _ProjectList({required this.workspace, required this.connection});

  @override
  Widget build(BuildContext context) {
    final folders = workspace.folders;
    final projectOrder = workspace.projectOrder;
    final projects = workspace.projects;

    // Build ordered list: folders and standalone projects
    final List<Widget> items = [];

    if (projectOrder.isNotEmpty || folders.isNotEmpty) {
      // Use project_order to display in correct order
      final folderMap = {for (final f in folders) f.id: f};
      final projectMap = {for (final p in projects) p.id: p};
      final displayedProjectIds = <String>{};

      for (final entryId in projectOrder) {
        final folder = folderMap[entryId];
        if (folder != null) {
          items.add(_FolderTile(
            folder: folder,
            projects: folder.projectIds
                .map((pid) => projectMap[pid])
                .whereType<state_ffi.ProjectInfo>()
                .toList(),
            workspace: workspace,
            connection: connection,
          ));
          displayedProjectIds.addAll(folder.projectIds);
        } else {
          final project = projectMap[entryId];
          if (project != null) {
            items.add(_ProjectTile(
              project: project,
              workspace: workspace,
              connection: connection,
            ));
            displayedProjectIds.add(entryId);
          }
        }
      }

      // Add any projects not in the order
      for (final p in projects) {
        if (!displayedProjectIds.contains(p.id)) {
          items.add(_ProjectTile(
            project: p,
            workspace: workspace,
            connection: connection,
          ));
        }
      }
    } else {
      // No ordering info — just list projects
      for (final p in projects) {
        items.add(_ProjectTile(
          project: p,
          workspace: workspace,
          connection: connection,
        ));
      }
    }

    return ListView(children: items);
  }
}

Color _folderColorToColor(String colorName) {
  switch (colorName) {
    case 'red':
      return Colors.red;
    case 'orange':
      return Colors.orange;
    case 'yellow':
      return Colors.yellow;
    case 'lime':
      return Colors.lime;
    case 'green':
      return Colors.green;
    case 'teal':
      return Colors.teal;
    case 'cyan':
      return Colors.cyan;
    case 'blue':
      return Colors.blue;
    case 'purple':
      return Colors.purple;
    case 'pink':
      return Colors.pink;
    default:
      return Colors.grey;
  }
}

class _FolderTile extends StatelessWidget {
  final state_ffi.FolderInfo folder;
  final List<state_ffi.ProjectInfo> projects;
  final WorkspaceProvider workspace;
  final ConnectionProvider connection;

  const _FolderTile({
    required this.folder,
    required this.projects,
    required this.workspace,
    required this.connection,
  });

  @override
  Widget build(BuildContext context) {
    final color = _folderColorToColor(folder.folderColor);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.only(left: 16, right: 16, top: 12, bottom: 4),
          child: Row(
            children: [
              Icon(Icons.folder, size: 18, color: color),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  folder.name,
                  style: TextStyle(
                    fontSize: 12,
                    fontWeight: FontWeight.w600,
                    color: color,
                    letterSpacing: 0.5,
                  ),
                ),
              ),
            ],
          ),
        ),
        ...projects.map((p) => _ProjectTile(
              project: p,
              workspace: workspace,
              connection: connection,
              indent: true,
            )),
      ],
    );
  }
}

class _ProjectTile extends StatelessWidget {
  final state_ffi.ProjectInfo project;
  final WorkspaceProvider workspace;
  final ConnectionProvider connection;
  final bool indent;

  const _ProjectTile({
    required this.project,
    required this.workspace,
    required this.connection,
    this.indent = false,
  });

  @override
  Widget build(BuildContext context) {
    final isSelected = project.id == workspace.selectedProjectId;
    final folderColor = _folderColorToColor(project.folderColor);

    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ListTile(
          contentPadding: EdgeInsets.only(left: indent ? 32 : 16, right: 16),
          leading: Icon(
            Icons.folder,
            color: isSelected
                ? Theme.of(context).colorScheme.primary
                : folderColor,
          ),
          title: Text(project.name),
          subtitle: _buildSubtitle(context),
          selected: isSelected,
          onTap: () {
            workspace.selectProject(project.id);
          },
        ),
        if (isSelected) ...[
          // Git status
          if (project.gitBranch != null)
            _GitStatusRow(project: project),
          // Services
          if (project.services.isNotEmpty)
            ...project.services.map((s) => _ServiceRow(
                  service: s,
                  project: project,
                  connection: connection,
                )),
          // Terminals
          ...project.terminalIds.asMap().entries.map((entry) {
            final idx = entry.key;
            final tid = entry.value;
            final isTerminalSelected = tid == workspace.selectedTerminalId;
            final name =
                project.terminalNames[tid] ?? 'Terminal ${idx + 1}';
            return ListTile(
              contentPadding:
                  const EdgeInsets.only(left: 56, right: 8),
              leading: Icon(
                Icons.terminal,
                size: 20,
                color: isTerminalSelected
                    ? Theme.of(context).colorScheme.primary
                    : null,
              ),
              title: Text(
                name,
                style: TextStyle(
                  fontSize: 14,
                  color: isTerminalSelected
                      ? Theme.of(context).colorScheme.primary
                      : null,
                ),
              ),
              selected: isTerminalSelected,
              dense: true,
              trailing: _TerminalActions(
                connId: connection.connId!,
                projectId: project.id,
                terminalId: tid,
                name: name,
              ),
              onTap: () {
                workspace.selectTerminal(tid);
                Navigator.of(context).pop();
              },
              onLongPress: () {
                _showTerminalMenu(
                  context,
                  connId: connection.connId!,
                  projectId: project.id,
                  terminalId: tid,
                  name: name,
                );
              },
            );
          }),
          if (connection.connId != null)
            ListTile(
              contentPadding:
                  const EdgeInsets.only(left: 56, right: 16),
              leading: Icon(
                Icons.add,
                size: 20,
                color:
                    Theme.of(context).colorScheme.onSurfaceVariant,
              ),
              title: Text(
                'New Terminal',
                style: TextStyle(
                  fontSize: 14,
                  color: Theme.of(context)
                      .colorScheme
                      .onSurfaceVariant,
                ),
              ),
              dense: true,
              onTap: () {
                state_ffi.createTerminal(
                  connId: connection.connId!,
                  projectId: project.id,
                );
                Navigator.of(context).pop();
              },
            ),
        ],
      ],
    );
  }

  Widget? _buildSubtitle(BuildContext context) {
    final parts = <Widget>[];
    if (project.gitBranch != null) {
      parts.add(Icon(Icons.commit, size: 12, color: Colors.grey[500]));
      parts.add(const SizedBox(width: 2));
      parts.add(Text(
        project.gitBranch!,
        style: TextStyle(fontSize: 11, color: Colors.grey[500]),
      ));
    }
    final runningServices =
        project.services.where((s) => s.status == 'running').length;
    if (runningServices > 0) {
      if (parts.isNotEmpty) parts.add(const SizedBox(width: 8));
      parts.add(Icon(Icons.dns, size: 12, color: Colors.green[400]));
      parts.add(const SizedBox(width: 2));
      parts.add(Text(
        '$runningServices',
        style: TextStyle(fontSize: 11, color: Colors.green[400]),
      ));
    }
    if (parts.isEmpty) return null;
    return Row(children: parts);
  }

  void _showTerminalMenu(
    BuildContext context, {
    required String connId,
    required String projectId,
    required String terminalId,
    required String name,
  }) {
    showModalBottomSheet(
      context: context,
      builder: (ctx) => SafeArea(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            ListTile(
              leading: const Icon(Icons.edit),
              title: const Text('Rename'),
              onTap: () {
                Navigator.of(ctx).pop();
                _showRenameDialog(context,
                    connId: connId,
                    projectId: projectId,
                    terminalId: terminalId,
                    currentName: name);
              },
            ),
            ListTile(
              leading: const Icon(Icons.close, color: Colors.redAccent),
              title:
                  const Text('Close', style: TextStyle(color: Colors.redAccent)),
              onTap: () {
                Navigator.of(ctx).pop();
                Navigator.of(context).pop();
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
    BuildContext context, {
    required String connId,
    required String projectId,
    required String terminalId,
    required String currentName,
  }) {
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

class _TerminalActions extends StatelessWidget {
  final String connId;
  final String projectId;
  final String terminalId;
  final String name;

  const _TerminalActions({
    required this.connId,
    required this.projectId,
    required this.terminalId,
    required this.name,
  });

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      width: 32,
      height: 32,
      child: IconButton(
        padding: EdgeInsets.zero,
        iconSize: 16,
        icon: const Icon(Icons.close, size: 16),
        onPressed: () {
          Navigator.of(context).pop();
          state_ffi.closeTerminal(
            connId: connId,
            projectId: projectId,
            terminalId: terminalId,
          );
        },
      ),
    );
  }
}

class _GitStatusRow extends StatelessWidget {
  final state_ffi.ProjectInfo project;

  const _GitStatusRow({required this.project});

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(left: 56, right: 16, bottom: 4),
      child: Row(
        children: [
          Icon(Icons.commit, size: 16, color: Colors.grey[400]),
          const SizedBox(width: 6),
          Text(
            project.gitBranch ?? '',
            style: TextStyle(fontSize: 12, color: Colors.grey[400]),
          ),
          const Spacer(),
          if (project.gitLinesAdded > 0) ...[
            Text(
              '+${project.gitLinesAdded}',
              style: TextStyle(fontSize: 11, color: Colors.green[400]),
            ),
            const SizedBox(width: 4),
          ],
          if (project.gitLinesRemoved > 0)
            Text(
              '-${project.gitLinesRemoved}',
              style: TextStyle(fontSize: 11, color: Colors.red[400]),
            ),
        ],
      ),
    );
  }
}

class _ServiceRow extends StatelessWidget {
  final state_ffi.ServiceInfo service;
  final state_ffi.ProjectInfo project;
  final ConnectionProvider connection;

  const _ServiceRow({
    required this.service,
    required this.project,
    required this.connection,
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

  IconData _statusIcon() {
    switch (service.status) {
      case 'running':
        return Icons.check_circle;
      case 'stopped':
        return Icons.stop_circle;
      case 'crashed':
        return Icons.error;
      case 'starting':
      case 'restarting':
        return Icons.hourglass_top;
      default:
        return Icons.help;
    }
  }

  @override
  Widget build(BuildContext context) {
    final connId = connection.connId;
    final color = _statusColor();

    return ListTile(
      contentPadding: const EdgeInsets.only(left: 56, right: 8),
      leading: Icon(_statusIcon(), size: 18, color: color),
      title: Row(
        children: [
          Expanded(
            child: Text(
              service.name,
              style: const TextStyle(fontSize: 13),
            ),
          ),
          if (service.ports.isNotEmpty)
            Text(
              service.ports.map((p) => ':$p').join(' '),
              style: TextStyle(fontSize: 11, color: Colors.grey[500]),
            ),
        ],
      ),
      dense: true,
      trailing: _ServiceActionButton(
        service: service,
        connId: connId,
        projectId: project.id,
      ),
    );
  }
}

class _ServiceActionButton extends StatelessWidget {
  final state_ffi.ServiceInfo service;
  final String? connId;
  final String projectId;

  const _ServiceActionButton({
    required this.service,
    required this.connId,
    required this.projectId,
  });

  @override
  Widget build(BuildContext context) {
    if (connId == null) return const SizedBox.shrink();

    return PopupMenuButton<String>(
      padding: EdgeInsets.zero,
      iconSize: 20,
      itemBuilder: (ctx) => [
        if (service.status == 'stopped' || service.status == 'crashed')
          const PopupMenuItem(value: 'start', child: Text('Start')),
        if (service.status == 'running')
          const PopupMenuItem(value: 'stop', child: Text('Stop')),
        if (service.status == 'running')
          const PopupMenuItem(value: 'restart', child: Text('Restart')),
      ],
      onSelected: (action) {
        switch (action) {
          case 'start':
            state_ffi.startService(
              connId: connId!,
              projectId: projectId,
              serviceName: service.name,
            );
            break;
          case 'stop':
            state_ffi.stopService(
              connId: connId!,
              projectId: projectId,
              serviceName: service.name,
            );
            break;
          case 'restart':
            state_ffi.restartService(
              connId: connId!,
              projectId: projectId,
              serviceName: service.name,
            );
            break;
        }
      },
    );
  }
}
