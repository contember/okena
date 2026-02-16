import 'dart:ui';

import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../providers/connection_provider.dart';
import '../providers/workspace_provider.dart';
import '../widgets/project_drawer.dart';
import '../widgets/key_toolbar.dart' show KeyToolbar, KeyModifiers;
import '../widgets/terminal_view.dart';
import '../theme/app_theme.dart';
import '../../src/rust/api/state.dart' as ffi;

class WorkspaceScreen extends StatefulWidget {
  const WorkspaceScreen({super.key});

  @override
  State<WorkspaceScreen> createState() => _WorkspaceScreenState();
}

class _WorkspaceScreenState extends State<WorkspaceScreen> {
  late PageController _pageController;
  final _keyModifiers = KeyModifiers();
  int _currentPage = 0;
  String? _lastProjectId;

  @override
  void initState() {
    super.initState();
    _pageController = PageController();
  }

  @override
  void dispose() {
    _pageController.dispose();
    _keyModifiers.dispose();
    super.dispose();
  }

  void _syncState(String projectId, List<String> terminalIds) {
    if (projectId != _lastProjectId) {
      _lastProjectId = projectId;
      _currentPage = 0;
      if (_pageController.hasClients) {
        _pageController.jumpToPage(0);
      }
    }
    if (_currentPage >= terminalIds.length && terminalIds.isNotEmpty) {
      _currentPage = terminalIds.length - 1;
    }
  }

  @override
  Widget build(BuildContext context) {
    final workspace = context.watch<WorkspaceProvider>();
    final connection = context.watch<ConnectionProvider>();
    final project = workspace.selectedProject;
    final connId = connection.connId;

    if (connId == null || project == null) {
      return Scaffold(
        backgroundColor: OkenaColors.background,
        body: Center(
          child: Text(
            'No project selected',
            style: OkenaTypography.callout.copyWith(color: OkenaColors.textTertiary),
          ),
        ),
      );
    }

    final terminalIds = project.terminalIds;
    _syncState(project.id, terminalIds);

    final safeCurrentPage = terminalIds.isNotEmpty
        ? _currentPage.clamp(0, terminalIds.length - 1)
        : 0;
    final currentTerminalId = terminalIds.isNotEmpty
        ? terminalIds[safeCurrentPage]
        : null;

    return Scaffold(
      backgroundColor: OkenaColors.background,
      body: SafeArea(
        bottom: false,
        child: Column(
          children: [
            _Header(
              projectName: project.name,
              folderColor: project.folderColor,
              terminalCount: terminalIds.length,
              currentPage: safeCurrentPage,
              onCreateTerminal: () {
                HapticFeedback.mediumImpact();
                ffi.createTerminal(connId: connId, projectId: project.id);
              },
              onCloseTerminal: currentTerminalId != null
                  ? () {
                      HapticFeedback.mediumImpact();
                      ffi.closeTerminal(
                        connId: connId,
                        projectId: project.id,
                        terminalId: currentTerminalId,
                      );
                    }
                  : null,
            ),
            Expanded(
              child: terminalIds.isEmpty
                  ? _buildEmptyState(connId, project.id)
                  : PageView.builder(
                      controller: _pageController,
                      itemCount: terminalIds.length,
                      onPageChanged: (i) => setState(() => _currentPage = i),
                      itemBuilder: (context, index) => TerminalView(
                        connId: connId,
                        terminalId: terminalIds[index],
                        modifiers: _keyModifiers,
                      ),
                    ),
            ),
            KeyToolbar(
              connId: connId,
              terminalId: currentTerminalId,
              modifiers: _keyModifiers,
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildEmptyState(String connId, String projectId) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(Icons.terminal_rounded, color: OkenaColors.textTertiary.withOpacity(0.3), size: 48),
          const SizedBox(height: 12),
          Text(
            'No terminals',
            style: OkenaTypography.callout.copyWith(color: OkenaColors.textTertiary),
          ),
          const SizedBox(height: 16),
          GestureDetector(
            onTap: () {
              HapticFeedback.mediumImpact();
              ffi.createTerminal(connId: connId, projectId: projectId);
            },
            child: Container(
              padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
              decoration: BoxDecoration(
                color: OkenaColors.accent.withOpacity(0.15),
                borderRadius: BorderRadius.circular(8),
                border: Border.all(color: OkenaColors.accent.withOpacity(0.3), width: 0.5),
              ),
              child: Text(
                'New Terminal',
                style: OkenaTypography.callout.copyWith(color: OkenaColors.accent),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

// ── Header with frosted glass ──────────────────────────────────────────

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

class _Header extends StatelessWidget {
  final String projectName;
  final String folderColor;
  final int terminalCount;
  final int currentPage;
  final VoidCallback onCreateTerminal;
  final VoidCallback? onCloseTerminal;

  const _Header({
    required this.projectName,
    required this.folderColor,
    required this.terminalCount,
    required this.currentPage,
    required this.onCreateTerminal,
    this.onCloseTerminal,
  });

  @override
  Widget build(BuildContext context) {
    final color = _folderColorToColor(folderColor);

    return ClipRect(
      child: BackdropFilter(
        filter: ImageFilter.blur(sigmaX: 24, sigmaY: 24),
        child: Container(
          height: 44,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: const BoxDecoration(
            color: OkenaColors.glassBg,
            border: Border(
              bottom: BorderSide(color: OkenaColors.glassStroke, width: 0.5),
            ),
          ),
          child: Row(
            children: [
              // Project avatar
              Container(
                width: 28,
                height: 28,
                decoration: BoxDecoration(
                  color: color.withOpacity(0.15),
                  borderRadius: BorderRadius.circular(7),
                ),
                alignment: Alignment.center,
                child: Text(
                  projectName.isNotEmpty ? projectName[0].toUpperCase() : '?',
                  style: TextStyle(
                    color: color,
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                  ),
                ),
              ),
              const SizedBox(width: 10),
              // Project name + chevron (tappable → opens project sheet)
              Expanded(
                child: GestureDetector(
                  onTap: () {
                    HapticFeedback.selectionClick();
                    ProjectSheet.show(context);
                  },
                  child: Row(
                    mainAxisSize: MainAxisSize.min,
                    children: [
                      Flexible(
                        child: Text(
                          projectName,
                          style: OkenaTypography.callout.copyWith(
                            color: OkenaColors.textPrimary,
                            fontWeight: FontWeight.w600,
                          ),
                          overflow: TextOverflow.ellipsis,
                        ),
                      ),
                      const SizedBox(width: 4),
                      const Icon(
                        CupertinoIcons.chevron_down,
                        color: OkenaColors.textTertiary,
                        size: 12,
                      ),
                    ],
                  ),
                ),
              ),
              // Page indicator dots (only when >1 terminal)
              if (terminalCount > 1) ...[
                const SizedBox(width: 8),
                Row(
                  mainAxisSize: MainAxisSize.min,
                  children: List.generate(terminalCount, (i) {
                    final isActive = i == currentPage;
                    return Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 2.5),
                      child: AnimatedContainer(
                        duration: const Duration(milliseconds: 200),
                        width: isActive ? 8 : 6,
                        height: isActive ? 8 : 6,
                        decoration: BoxDecoration(
                          color: isActive ? OkenaColors.accent : OkenaColors.textTertiary,
                          shape: BoxShape.circle,
                        ),
                      ),
                    );
                  }),
                ),
              ],
              const SizedBox(width: 8),
              // Close terminal button
              if (onCloseTerminal != null)
                GestureDetector(
                  onTap: onCloseTerminal,
                  child: Container(
                    width: 28,
                    height: 28,
                    decoration: BoxDecoration(
                      color: OkenaColors.surfaceElevated,
                      borderRadius: BorderRadius.circular(7),
                    ),
                    alignment: Alignment.center,
                    child: const Icon(
                      Icons.close_rounded,
                      color: OkenaColors.textTertiary,
                      size: 14,
                    ),
                  ),
                ),
              const SizedBox(width: 4),
              // Create terminal button
              GestureDetector(
                onTap: onCreateTerminal,
                child: Container(
                  width: 28,
                  height: 28,
                  decoration: BoxDecoration(
                    color: OkenaColors.surfaceElevated,
                    borderRadius: BorderRadius.circular(7),
                  ),
                  alignment: Alignment.center,
                  child: const Icon(
                    Icons.add_rounded,
                    color: OkenaColors.textSecondary,
                    size: 16,
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
