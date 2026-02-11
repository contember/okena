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
        title: Text(project?.name ?? 'No Project'),
        leading: Builder(
          builder: (ctx) => IconButton(
            icon: const Icon(Icons.menu),
            onPressed: () => Scaffold.of(ctx).openDrawer(),
          ),
        ),
        actions: [
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
                    Expanded(
                      child: TerminalView(
                        connId: connId,
                        terminalId: selectedTerminalId,
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
