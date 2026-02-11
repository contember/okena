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
            child: ListView.builder(
              itemCount: workspace.projects.length,
              itemBuilder: (context, index) {
                final project = workspace.projects[index];
                final isSelected = project.id == workspace.selectedProjectId;
                return Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    ListTile(
                      leading: Icon(
                        Icons.folder,
                        color: isSelected
                            ? Theme.of(context).colorScheme.primary
                            : null,
                      ),
                      title: Text(project.name),
                      subtitle: Text(
                        project.path,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: Theme.of(context).textTheme.bodySmall,
                      ),
                      selected: isSelected,
                      onTap: () {
                        workspace.selectProject(project.id);
                      },
                    ),
                    if (isSelected) ...[
                      ...project.terminalIds.map((tid) {
                        final isTerminalSelected =
                            tid == workspace.selectedTerminalId;
                        final name =
                            project.terminalNames[tid] ?? 'Terminal';
                        return ListTile(
                          contentPadding:
                              const EdgeInsets.only(left: 56, right: 16),
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
                          onTap: () {
                            workspace.selectTerminal(tid);
                            Navigator.of(context).pop();
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
              },
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
