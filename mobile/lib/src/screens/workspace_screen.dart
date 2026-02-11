import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/connection_provider.dart';
import '../providers/workspace_provider.dart';
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
      ),
      drawer: const ProjectDrawer(),
      body: connId == null || project == null
          ? const Center(child: Text('No project selected'))
          : selectedTerminalId == null
              ? const Center(
                  child: Text(
                    'No terminals',
                    style: TextStyle(color: Colors.grey),
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
