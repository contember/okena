import 'dart:convert';

import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart' show Uint64List;
import 'package:provider/provider.dart';

import '../providers/connection_provider.dart';
import '../providers/workspace_provider.dart';
import '../rust/api/state.dart' as state_ffi;
import '../widgets/project_drawer.dart';
import '../widgets/key_toolbar.dart';
import '../widgets/terminal_view.dart';

class WorkspaceScreen extends StatefulWidget {
  const WorkspaceScreen({super.key});

  @override
  State<WorkspaceScreen> createState() => _WorkspaceScreenState();
}

class _WorkspaceScreenState extends State<WorkspaceScreen> {
  final KeyModifiers _modifiers = KeyModifiers();

  @override
  void dispose() {
    _modifiers.dispose();
    super.dispose();
  }

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
          // More actions (split, minimize, new terminal)
          if (connId != null && project != null)
            PopupMenuButton<String>(
              icon: const Icon(Icons.add, size: 22),
              tooltip: 'Terminal actions',
              itemBuilder: (ctx) => [
                const PopupMenuItem(
                  value: 'new',
                  child: ListTile(
                    leading: Icon(Icons.add, size: 20),
                    title: Text('New Terminal'),
                    dense: true,
                    contentPadding: EdgeInsets.zero,
                  ),
                ),
                if (selectedTerminalId != null) ...[
                  const PopupMenuItem(
                    value: 'split_vertical',
                    child: ListTile(
                      leading: Icon(Icons.vertical_split, size: 20),
                      title: Text('Split Vertical'),
                      dense: true,
                      contentPadding: EdgeInsets.zero,
                    ),
                  ),
                  const PopupMenuItem(
                    value: 'split_horizontal',
                    child: ListTile(
                      leading: Icon(Icons.horizontal_split, size: 20),
                      title: Text('Split Horizontal'),
                      dense: true,
                      contentPadding: EdgeInsets.zero,
                    ),
                  ),
                  const PopupMenuItem(
                    value: 'minimize',
                    child: ListTile(
                      leading: Icon(Icons.minimize, size: 20),
                      title: Text('Minimize'),
                      dense: true,
                      contentPadding: EdgeInsets.zero,
                    ),
                  ),
                ],
              ],
              onSelected: (value) {
                switch (value) {
                  case 'new':
                    state_ffi.createTerminal(
                      connId: connId,
                      projectId: project.id,
                    );
                    break;
                  case 'split_vertical':
                    state_ffi.splitTerminal(
                      connId: connId,
                      projectId: project.id,
                      path: Uint64List.fromList([]),
                      direction: 'vertical',
                    );
                    break;
                  case 'split_horizontal':
                    state_ffi.splitTerminal(
                      connId: connId,
                      projectId: project.id,
                      path: Uint64List.fromList([]),
                      direction: 'horizontal',
                    );
                    break;
                  case 'minimize':
                    if (selectedTerminalId != null) {
                      state_ffi.toggleMinimized(
                        connId: connId,
                        projectId: project.id,
                        terminalId: selectedTerminalId,
                      );
                    }
                    break;
                }
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
                        modifiers: _modifiers,
                      ),
                    ),
                    KeyToolbar(
                      connId: connId,
                      terminalId: selectedTerminalId,
                      modifiers: _modifiers,
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
              leading: const Icon(Icons.vertical_split),
              title: const Text('Split Vertical'),
              onTap: () {
                Navigator.of(ctx).pop();
                state_ffi.splitTerminal(
                  connId: connId,
                  projectId: projectId,
                  path: Uint64List.fromList([]),
                  direction: 'vertical',
                );
              },
            ),
            ListTile(
              leading: const Icon(Icons.horizontal_split),
              title: const Text('Split Horizontal'),
              onTap: () {
                Navigator.of(ctx).pop();
                state_ffi.splitTerminal(
                  connId: connId,
                  projectId: projectId,
                  path: Uint64List.fromList([]),
                  direction: 'horizontal',
                );
              },
            ),
            ListTile(
              leading: const Icon(Icons.minimize),
              title: const Text('Minimize'),
              onTap: () {
                Navigator.of(ctx).pop();
                state_ffi.toggleMinimized(
                  connId: connId,
                  projectId: projectId,
                  terminalId: terminalId,
                );
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

class _GitSheetState extends State<_GitSheet> with SingleTickerProviderStateMixin {
  String? _diffSummary;
  String? _branches;
  String? _gitStatus;
  String? _workingTreeDiff;
  String? _stagedDiff;
  bool _loading = true;
  late TabController _tabController;

  @override
  void initState() {
    super.initState();
    _tabController = TabController(length: 4, vsync: this);
    _loadData();
  }

  @override
  void dispose() {
    _tabController.dispose();
    super.dispose();
  }

  Future<void> _loadData() async {
    try {
      final results = await Future.wait([
        state_ffi.gitDiffSummary(
            connId: widget.connId, projectId: widget.project.id),
        state_ffi.gitBranches(
            connId: widget.connId, projectId: widget.project.id),
        state_ffi.gitStatus(
            connId: widget.connId, projectId: widget.project.id),
        state_ffi.gitDiff(
            connId: widget.connId, projectId: widget.project.id, mode: 'working_tree'),
        state_ffi.gitDiff(
            connId: widget.connId, projectId: widget.project.id, mode: 'staged'),
      ]);
      if (mounted) {
        setState(() {
          _diffSummary = results[0];
          _branches = results[1];
          _gitStatus = results[2];
          _workingTreeDiff = results[3];
          _stagedDiff = results[4];
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
        // Tab bar
        TabBar(
          controller: _tabController,
          isScrollable: true,
          tabAlignment: TabAlignment.start,
          labelStyle: const TextStyle(fontSize: 13),
          tabs: const [
            Tab(text: 'Changes'),
            Tab(text: 'Diff'),
            Tab(text: 'Staged'),
            Tab(text: 'Branches'),
          ],
        ),
        Expanded(
          child: _loading
              ? const Center(child: CircularProgressIndicator())
              : TabBarView(
                  controller: _tabController,
                  children: [
                    // Changes tab
                    ListView(
                      controller: widget.scrollController,
                      padding: const EdgeInsets.all(16),
                      children: [
                        if (_diffSummary != null)
                          _DiffSummaryView(
                            json: _diffSummary!,
                            connId: widget.connId,
                            projectId: widget.project.id,
                          ),
                        if (_gitStatus != null) ...[
                          const SizedBox(height: 16),
                          Text('Status',
                              style: Theme.of(context).textTheme.titleSmall),
                          const SizedBox(height: 8),
                          _GitStatusView(json: _gitStatus!),
                        ],
                      ],
                    ),
                    // Working tree diff tab
                    _DiffContentView(
                      diff: _workingTreeDiff,
                      scrollController: widget.scrollController,
                    ),
                    // Staged diff tab
                    _DiffContentView(
                      diff: _stagedDiff,
                      scrollController: widget.scrollController,
                    ),
                    // Branches tab
                    ListView(
                      controller: widget.scrollController,
                      padding: const EdgeInsets.all(16),
                      children: [
                        if (_branches != null) _BranchesView(json: _branches!),
                      ],
                    ),
                  ],
                ),
        ),
      ],
    );
  }
}

/// Renders git status JSON.
class _GitStatusView extends StatelessWidget {
  final String json;

  const _GitStatusView({required this.json});

  @override
  Widget build(BuildContext context) {
    try {
      final data = jsonDecode(json);
      if (data is Map) {
        final entries = <Widget>[];

        void addSection(String title, dynamic files) {
          if (files is List && files.isNotEmpty) {
            entries.add(Padding(
              padding: const EdgeInsets.only(top: 8, bottom: 4),
              child: Text(
                title,
                style: TextStyle(
                  fontSize: 12,
                  fontWeight: FontWeight.w600,
                  color: Colors.grey[400],
                ),
              ),
            ));
            for (final f in files) {
              final path = f is String ? f : (f is Map ? f['path'] as String? ?? '' : f.toString());
              entries.add(Padding(
                padding: const EdgeInsets.symmetric(vertical: 2),
                child: Text(
                  path,
                  style: const TextStyle(
                    fontSize: 12,
                    fontFamily: 'JetBrainsMono',
                  ),
                ),
              ));
            }
          }
        }

        addSection('Staged', data['staged']);
        addSection('Modified', data['modified'] ?? data['unstaged']);
        addSection('Untracked', data['untracked']);

        if (entries.isEmpty) {
          return Text('Clean working tree',
              style: TextStyle(color: Colors.grey[500]));
        }
        return Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: entries,
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

/// Full diff content viewer with syntax-colored diff lines.
class _DiffContentView extends StatelessWidget {
  final String? diff;
  final ScrollController scrollController;

  const _DiffContentView({
    required this.diff,
    required this.scrollController,
  });

  @override
  Widget build(BuildContext context) {
    if (diff == null || diff!.isEmpty) {
      return Center(
        child: Text('No changes', style: TextStyle(color: Colors.grey[500])),
      );
    }

    final lines = diff!.split('\n');
    return ListView.builder(
      controller: scrollController,
      itemCount: lines.length,
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 4),
      itemBuilder: (context, index) {
        final line = lines[index];
        Color? bgColor;
        Color textColor = Colors.grey[300]!;

        if (line.startsWith('+')) {
          bgColor = Colors.green.withValues(alpha: 0.1);
          textColor = Colors.green[300]!;
        } else if (line.startsWith('-')) {
          bgColor = Colors.red.withValues(alpha: 0.1);
          textColor = Colors.red[300]!;
        } else if (line.startsWith('@@')) {
          textColor = Colors.cyan[300]!;
        } else if (line.startsWith('diff ') || line.startsWith('index ')) {
          textColor = Colors.grey[500]!;
        }

        return Container(
          color: bgColor,
          padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
          child: Text(
            line,
            style: TextStyle(
              fontSize: 11,
              fontFamily: 'JetBrainsMono',
              color: textColor,
            ),
          ),
        );
      },
    );
  }
}

class _DiffSummaryView extends StatelessWidget {
  final String json;
  final String? connId;
  final String? projectId;

  const _DiffSummaryView({
    required this.json,
    this.connId,
    this.projectId,
  });

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

            return InkWell(
              onTap: connId != null && projectId != null
                  ? () => _showFileContents(context, path)
                  : null,
              child: Padding(
                padding: const EdgeInsets.symmetric(vertical: 4),
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

  void _showFileContents(BuildContext context, String filePath) {
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      builder: (ctx) => DraggableScrollableSheet(
        initialChildSize: 0.8,
        minChildSize: 0.3,
        maxChildSize: 0.95,
        expand: false,
        builder: (ctx, scrollController) => _FileContentsSheet(
          connId: connId!,
          projectId: projectId!,
          filePath: filePath,
          scrollController: scrollController,
        ),
      ),
    );
  }
}

class _FileContentsSheet extends StatefulWidget {
  final String connId;
  final String projectId;
  final String filePath;
  final ScrollController scrollController;

  const _FileContentsSheet({
    required this.connId,
    required this.projectId,
    required this.filePath,
    required this.scrollController,
  });

  @override
  State<_FileContentsSheet> createState() => _FileContentsSheetState();
}

class _FileContentsSheetState extends State<_FileContentsSheet> {
  String? _contents;
  String? _error;
  bool _loading = true;

  @override
  void initState() {
    super.initState();
    _loadContents();
  }

  Future<void> _loadContents() async {
    try {
      final contents = await state_ffi.gitFileContents(
        connId: widget.connId,
        projectId: widget.projectId,
        filePath: widget.filePath,
        mode: 'working_tree',
      );
      if (mounted) {
        setState(() {
          _contents = contents;
          _loading = false;
        });
      }
    } catch (e) {
      if (mounted) {
        setState(() {
          _error = e.toString();
          _loading = false;
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
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
          padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
          child: Row(
            children: [
              const Icon(Icons.description, size: 20),
              const SizedBox(width: 8),
              Expanded(
                child: Text(
                  widget.filePath,
                  style: const TextStyle(
                    fontSize: 13,
                    fontFamily: 'JetBrainsMono',
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
        ),
        const Divider(),
        Expanded(
          child: _loading
              ? const Center(child: CircularProgressIndicator())
              : _error != null
                  ? Center(
                      child: Padding(
                        padding: const EdgeInsets.all(16),
                        child: Text(
                          _error!,
                          style: TextStyle(color: Colors.red[400]),
                        ),
                      ),
                    )
                  : SingleChildScrollView(
                      controller: widget.scrollController,
                      scrollDirection: Axis.horizontal,
                      child: SingleChildScrollView(
                        child: Padding(
                          padding: const EdgeInsets.all(16),
                          child: Text(
                            _contents ?? '',
                            style: const TextStyle(
                              fontSize: 12,
                              fontFamily: 'JetBrainsMono',
                            ),
                          ),
                        ),
                      ),
                    ),
        ),
      ],
    );
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
