import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../providers/connection_provider.dart';
import '../providers/workspace_provider.dart';
import '../../src/rust/api/state.dart' as ffi;
import '../theme/app_theme.dart';
import 'status_indicator.dart';

Color _folderColorToColor(String colorName) {
  return switch (colorName) {
    'red' => const Color(0xFFEF4444),
    'orange' => const Color(0xFFF97316),
    'yellow' => const Color(0xFFEAB308),
    'lime' => const Color(0xFF84CC16),
    'green' => const Color(0xFF22C55E),
    'teal' => const Color(0xFF14B8A6),
    'cyan' => const Color(0xFF06B6D4),
    'blue' => const Color(0xFF3B82F6),
    'indigo' => const Color(0xFF6366F1),
    'purple' => const Color(0xFFA855F7),
    'pink' => const Color(0xFFEC4899),
    _ => OkenaColors.textTertiary,
  };
}

/// Bottom sheet for project selection (replaces old Drawer).
class ProjectSheet extends StatelessWidget {
  const ProjectSheet({super.key});

  static void show(BuildContext context) {
    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: Colors.transparent,
      builder: (_) => DraggableScrollableSheet(
        initialChildSize: 0.6,
        minChildSize: 0.3,
        maxChildSize: 0.9,
        snap: true,
        snapSizes: const [0.3, 0.6, 0.9],
        builder: (context, scrollController) => _SheetContent(
          scrollController: scrollController,
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    return const SizedBox.shrink();
  }
}

class _SheetContent extends StatelessWidget {
  final ScrollController scrollController;

  const _SheetContent({required this.scrollController});

  @override
  Widget build(BuildContext context) {
    final workspace = context.watch<WorkspaceProvider>();
    final connection = context.watch<ConnectionProvider>();

    return Container(
      decoration: const BoxDecoration(
        color: OkenaColors.surface,
        borderRadius: BorderRadius.vertical(top: Radius.circular(16)),
      ),
      child: Column(
        children: [
          // Drag handle
          Center(
            child: Container(
              width: 36,
              height: 4,
              margin: const EdgeInsets.only(top: 10, bottom: 16),
              decoration: BoxDecoration(
                color: OkenaColors.textTertiary.withOpacity(0.4),
                borderRadius: BorderRadius.circular(2),
              ),
            ),
          ),
          // Header
          Padding(
            padding: const EdgeInsets.fromLTRB(20, 0, 20, 16),
            child: Row(
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      const Text('Projects', style: OkenaTypography.title),
                      if (connection.activeServer != null) ...[
                        const SizedBox(height: 3),
                        Text(
                          connection.activeServer!.displayName,
                          style: OkenaTypography.caption2.copyWith(
                            color: OkenaColors.textTertiary,
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
                StatusIndicator(status: connection.status),
              ],
            ),
          ),
          // Divider
          Container(
            height: 0.5,
            color: OkenaColors.border,
          ),
          // Project list
          Expanded(
            child: _ProjectList(
              workspace: workspace,
              scrollController: scrollController,
            ),
          ),
          // Disconnect footer
          Container(
            decoration: const BoxDecoration(
              border: Border(
                top: BorderSide(color: OkenaColors.border, width: 0.5),
              ),
            ),
            child: SafeArea(
              top: false,
              child: InkWell(
                onTap: () {
                  HapticFeedback.mediumImpact();
                  Navigator.of(context).pop();
                  connection.disconnect();
                },
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 14),
                  child: Row(
                    children: [
                      Icon(
                        Icons.link_off_rounded,
                        color: OkenaColors.error.withOpacity(0.7),
                        size: 16,
                      ),
                      const SizedBox(width: 10),
                      Text(
                        'Disconnect',
                        style: OkenaTypography.callout.copyWith(
                          color: OkenaColors.error.withOpacity(0.8),
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// ── Project list with folder grouping ──────────────────────────────

class _ProjectList extends StatelessWidget {
  final WorkspaceProvider workspace;
  final ScrollController scrollController;

  const _ProjectList({
    required this.workspace,
    required this.scrollController,
  });

  @override
  Widget build(BuildContext context) {
    final projectMap = {for (final p in workspace.projects) p.id: p};
    final folderMap = {for (final f in workspace.folders) f.id: f};
    final projectOrder = workspace.projectOrder;

    // If no project_order, fall back to flat list
    if (projectOrder.isEmpty) {
      return ListView.builder(
        controller: scrollController,
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        itemCount: workspace.projects.length,
        itemBuilder: (context, index) {
          final project = workspace.projects[index];
          return _buildProjectTile(context, project);
        },
      );
    }

    // Build ordered items: folders expand to header + children, standalone projects inline
    final widgets = <Widget>[];
    for (final id in projectOrder) {
      final folder = folderMap[id];
      if (folder != null) {
        // Folder header
        widgets.add(_FolderHeader(
          name: folder.name,
          folderColor: _folderColorToColor(folder.folderColor),
        ));
        // Folder's projects
        for (final pid in folder.projectIds) {
          final project = projectMap[pid];
          if (project != null) {
            widgets.add(Padding(
              padding: const EdgeInsets.only(left: 16),
              child: _buildProjectTile(context, project),
            ));
          }
        }
      } else {
        final project = projectMap[id];
        if (project != null) {
          widgets.add(_buildProjectTile(context, project));
        }
      }
    }

    return ListView(
      controller: scrollController,
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      children: widgets,
    );
  }

  Widget _buildProjectTile(BuildContext context, ffi.ProjectInfo project) {
    final isSelected = project.id == workspace.selectedProjectId;
    return _ProjectTile(
      name: project.name,
      path: project.path,
      isSelected: isSelected,
      folderColor: _folderColorToColor(project.folderColor),
      onTap: () {
        HapticFeedback.selectionClick();
        workspace.selectProject(project.id);
        Navigator.of(context).pop();
      },
    );
  }
}

// ── Folder header ──────────────────────────────────────────────────

class _FolderHeader extends StatelessWidget {
  final String name;
  final Color folderColor;

  const _FolderHeader({
    required this.name,
    required this.folderColor,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.fromLTRB(12, 14, 12, 6),
      child: Row(
        children: [
          Container(
            width: 7,
            height: 7,
            decoration: BoxDecoration(
              color: folderColor == OkenaColors.textTertiary
                  ? OkenaColors.textTertiary
                  : folderColor.withOpacity(0.8),
              shape: BoxShape.circle,
            ),
          ),
          const SizedBox(width: 8),
          Text(
            name.toUpperCase(),
            style: TextStyle(
              color: folderColor == OkenaColors.textTertiary
                  ? OkenaColors.textTertiary
                  : folderColor.withOpacity(0.7),
              fontSize: 10,
              fontWeight: FontWeight.w700,
              letterSpacing: 1.0,
            ),
          ),
        ],
      ),
    );
  }
}

// ── Project tile ──────────────────────────────────────────────────────

class _ProjectTile extends StatelessWidget {
  final String name;
  final String path;
  final bool isSelected;
  final Color folderColor;
  final VoidCallback onTap;

  const _ProjectTile({
    required this.name,
    required this.path,
    required this.isSelected,
    required this.folderColor,
    required this.onTap,
  });

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 1),
      child: Material(
        color: isSelected ? OkenaColors.surfaceOverlay : Colors.transparent,
        borderRadius: BorderRadius.circular(10),
        child: InkWell(
          borderRadius: BorderRadius.circular(10),
          splashColor: OkenaColors.accent.withOpacity(0.08),
          highlightColor: OkenaColors.accent.withOpacity(0.04),
          onTap: onTap,
          child: Container(
            padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
            decoration: isSelected
                ? BoxDecoration(
                    borderRadius: BorderRadius.circular(10),
                    border: Border(
                      left: BorderSide(
                        color: OkenaColors.accent,
                        width: 3,
                      ),
                    ),
                  )
                : null,
            child: Row(
              children: [
                // Letter avatar
                Container(
                  width: 24,
                  height: 24,
                  decoration: BoxDecoration(
                    color: (isSelected ? OkenaColors.accent : folderColor)
                        .withOpacity(0.15),
                    borderRadius: BorderRadius.circular(6),
                  ),
                  alignment: Alignment.center,
                  child: Text(
                    name.isNotEmpty ? name[0].toUpperCase() : '?',
                    style: TextStyle(
                      color: isSelected ? OkenaColors.accent : folderColor,
                      fontSize: 11,
                      fontWeight: FontWeight.w700,
                    ),
                  ),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        name,
                        style: OkenaTypography.callout.copyWith(
                          color: isSelected
                              ? OkenaColors.textPrimary
                              : OkenaColors.textSecondary,
                          fontWeight:
                              isSelected ? FontWeight.w600 : FontWeight.w400,
                        ),
                      ),
                      const SizedBox(height: 1),
                      Text(
                        path,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: OkenaTypography.caption2.copyWith(
                          fontFamily: 'JetBrainsMono',
                          fontSize: 10,
                        ),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}
