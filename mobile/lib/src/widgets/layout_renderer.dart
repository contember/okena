import 'package:flutter/material.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart' show Uint64List;

import '../../src/rust/api/state.dart' as state_ffi;
import '../models/layout_node.dart';
import '../theme/app_theme.dart';
import 'key_toolbar.dart' show KeyModifiers;
import 'terminal_view.dart';

class LayoutRenderer extends StatelessWidget {
  final String connId;
  final String projectId;
  final List<String> terminalIds;
  final KeyModifiers modifiers;

  const LayoutRenderer({
    super.key,
    required this.connId,
    required this.projectId,
    required this.terminalIds,
    required this.modifiers,
  });

  @override
  Widget build(BuildContext context) {
    final json = state_ffi.getProjectLayoutJson(
      connId: connId,
      projectId: projectId,
    );

    if (json != null) {
      final node = LayoutNode.fromJson(json);
      if (node != null) {
        return _buildNode(context, node, const []);
      }
    }

    // Fallback: show first terminal
    if (terminalIds.isEmpty) {
      return const Center(
        child: Text(
          'No terminals',
          style: TextStyle(color: OkenaColors.textTertiary),
        ),
      );
    }
    return TerminalView(connId: connId, terminalId: terminalIds.first, modifiers: modifiers);
  }

  Widget _buildNode(BuildContext context, LayoutNode node, List<int> path) {
    return switch (node) {
      TerminalNode(:final terminalId, :final minimized) => terminalId != null
          ? minimized
              ? _MinimizedTerminal(
                  connId: connId,
                  projectId: projectId,
                  terminalId: terminalId,
                )
              : TerminalView(connId: connId, terminalId: terminalId, modifiers: modifiers)
          : const Center(
              child:
                  Text('Empty terminal', style: TextStyle(color: OkenaColors.textTertiary)),
            ),
      SplitNode(:final direction, :final sizes, :final children) =>
        _buildSplit(context, direction, sizes, children, path),
      TabsNode(:final activeTab, :final children) =>
        _buildTabs(context, activeTab, children, path),
    };
  }

  Widget _buildSplit(
    BuildContext context,
    SplitDirection direction,
    List<double> sizes,
    List<LayoutNode> children,
    List<int> path,
  ) {
    if (children.isEmpty) {
      return const SizedBox.shrink();
    }

    // In portrait mode, force horizontal splits to vertical
    final isPortrait =
        MediaQuery.of(context).orientation == Orientation.portrait;
    final isVertical =
        direction == SplitDirection.vertical || (direction == SplitDirection.horizontal && isPortrait);

    return _ResizableSplit(
      connId: connId,
      projectId: projectId,
      path: path,
      isVertical: isVertical,
      sizes: sizes,
      children: children,
      builder: (node, index) => _buildNode(context, node, [...path, index]),
    );
  }

  Widget _buildTabs(
    BuildContext context,
    int activeTab,
    List<LayoutNode> children,
    List<int> path,
  ) {
    if (children.isEmpty) {
      return const SizedBox.shrink();
    }

    return _TabsWidget(
      connId: connId,
      projectId: projectId,
      path: path,
      activeTab: activeTab.clamp(0, children.length - 1),
      children: children,
      builder: (node, index) => _buildNode(context, node, [...path, index]),
    );
  }
}

/// A minimized terminal placeholder.
class _MinimizedTerminal extends StatelessWidget {
  final String connId;
  final String projectId;
  final String terminalId;

  const _MinimizedTerminal({
    required this.connId,
    required this.projectId,
    required this.terminalId,
  });

  @override
  Widget build(BuildContext context) {
    return GestureDetector(
      onTap: () {
        state_ffi.toggleMinimized(
          connId: connId,
          projectId: projectId,
          terminalId: terminalId,
        );
      },
      child: Container(
        height: 36,
        color: OkenaColors.surfaceElevated,
        padding: const EdgeInsets.symmetric(horizontal: 12),
        child: Row(
          children: [
            const Icon(Icons.terminal, size: 16, color: OkenaColors.textSecondary),
            const SizedBox(width: 8),
            Text(
              terminalId.length > 8
                  ? '...${terminalId.substring(terminalId.length - 8)}'
                  : terminalId,
              style: const TextStyle(
                fontSize: 12,
                color: OkenaColors.textSecondary,
                fontFamily: 'JetBrainsMono',
              ),
            ),
            const Spacer(),
            const Icon(Icons.expand_more, size: 16, color: OkenaColors.textTertiary),
          ],
        ),
      ),
    );
  }
}

/// Resizable split pane with draggable dividers.
class _ResizableSplit extends StatefulWidget {
  final String connId;
  final String projectId;
  final List<int> path;
  final bool isVertical;
  final List<double> sizes;
  final List<LayoutNode> children;
  final Widget Function(LayoutNode, int) builder;

  const _ResizableSplit({
    required this.connId,
    required this.projectId,
    required this.path,
    required this.isVertical,
    required this.sizes,
    required this.children,
    required this.builder,
  });

  @override
  State<_ResizableSplit> createState() => _ResizableSplitState();
}

class _ResizableSplitState extends State<_ResizableSplit> {
  late List<double> _sizes;

  @override
  void initState() {
    super.initState();
    _sizes = List.of(widget.sizes);
    _ensureSizes();
  }

  @override
  void didUpdateWidget(_ResizableSplit oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.sizes != widget.sizes) {
      _sizes = List.of(widget.sizes);
      _ensureSizes();
    }
  }

  void _ensureSizes() {
    while (_sizes.length < widget.children.length) {
      _sizes.add(1.0);
    }
  }

  void _onDividerDrag(int dividerIndex, double delta, double totalSize) {
    if (totalSize <= 0) return;
    final total = _sizes.reduce((a, b) => a + b);
    final fraction = delta / totalSize * total;

    setState(() {
      _sizes[dividerIndex] = (_sizes[dividerIndex] + fraction).clamp(0.1, total);
      _sizes[dividerIndex + 1] = (_sizes[dividerIndex + 1] - fraction).clamp(0.1, total);
    });
  }

  void _onDividerDragEnd() {
    state_ffi.updateSplitSizes(
      connId: widget.connId,
      projectId: widget.projectId,
      path: Uint64List.fromList(widget.path),
      sizes: _sizes.map((s) => s).toList(),
    );
  }

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, constraints) {
        final totalSize = widget.isVertical
            ? constraints.maxHeight
            : constraints.maxWidth;

        final flexChildren = <Widget>[];
        for (int i = 0; i < widget.children.length; i++) {
          final flex = i < _sizes.length ? _sizes[i].round().clamp(1, 1000) : 1;
          if (i > 0) {
            final dividerIndex = i - 1;
            flexChildren.add(
              GestureDetector(
                behavior: HitTestBehavior.opaque,
                onVerticalDragUpdate: widget.isVertical
                    ? (details) => _onDividerDrag(dividerIndex, details.delta.dy, totalSize)
                    : null,
                onHorizontalDragUpdate: !widget.isVertical
                    ? (details) => _onDividerDrag(dividerIndex, details.delta.dx, totalSize)
                    : null,
                onVerticalDragEnd: widget.isVertical ? (_) => _onDividerDragEnd() : null,
                onHorizontalDragEnd: !widget.isVertical ? (_) => _onDividerDragEnd() : null,
                child: MouseRegion(
                  cursor: widget.isVertical
                      ? SystemMouseCursors.resizeRow
                      : SystemMouseCursors.resizeColumn,
                  child: Container(
                    width: widget.isVertical ? double.infinity : 6,
                    height: widget.isVertical ? 6 : double.infinity,
                    color: Colors.transparent,
                    child: Center(
                      child: Container(
                        width: widget.isVertical ? 32 : 2,
                        height: widget.isVertical ? 2 : 32,
                        decoration: BoxDecoration(
                          color: OkenaColors.borderLight,
                          borderRadius: BorderRadius.circular(1),
                        ),
                      ),
                    ),
                  ),
                ),
              ),
            );
          }
          flexChildren.add(
            Expanded(flex: flex, child: widget.builder(widget.children[i], i)),
          );
        }

        return Flex(
          direction: widget.isVertical ? Axis.vertical : Axis.horizontal,
          children: flexChildren,
        );
      },
    );
  }
}

class _TabsWidget extends StatelessWidget {
  final String connId;
  final String projectId;
  final List<int> path;
  final int activeTab;
  final List<LayoutNode> children;
  final Widget Function(LayoutNode, int) builder;

  const _TabsWidget({
    required this.connId,
    required this.projectId,
    required this.path,
    required this.activeTab,
    required this.children,
    required this.builder,
  });

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Container(
          height: 32,
          color: OkenaColors.surfaceElevated,
          child: Row(
            children: [
              Expanded(
                child: SingleChildScrollView(
                  scrollDirection: Axis.horizontal,
                  child: Row(
                    children: [
                      for (int i = 0; i < children.length; i++)
                        GestureDetector(
                          onTap: () {
                            if (i != activeTab) {
                              state_ffi.setActiveTab(
                                connId: connId,
                                projectId: projectId,
                                path: Uint64List.fromList(path),
                                index: BigInt.from(i),
                              );
                            }
                          },
                          child: Container(
                            padding:
                                const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                            decoration: BoxDecoration(
                              border: Border(
                                bottom: BorderSide(
                                  color: i == activeTab
                                      ? OkenaColors.accent
                                      : Colors.transparent,
                                  width: 2,
                                ),
                              ),
                            ),
                            child: Text(
                              _tabLabel(children[i], i),
                              style: TextStyle(
                                color: i == activeTab ? OkenaColors.textPrimary : OkenaColors.textSecondary,
                                fontSize: 12,
                              ),
                            ),
                          ),
                        ),
                    ],
                  ),
                ),
              ),
              // Add tab button
              GestureDetector(
                onTap: () {
                  state_ffi.addTab(
                    connId: connId,
                    projectId: projectId,
                    path: Uint64List.fromList(path),
                    inGroup: true,
                  );
                },
                child: const Padding(
                  padding: EdgeInsets.symmetric(horizontal: 8),
                  child: Icon(Icons.add, size: 16, color: OkenaColors.textTertiary),
                ),
              ),
            ],
          ),
        ),
        Expanded(
          child: builder(children[activeTab], activeTab),
        ),
      ],
    );
  }

  String _tabLabel(LayoutNode node, int index) {
    if (node is TerminalNode && node.terminalId != null) {
      final id = node.terminalId!;
      return id.length > 6 ? '...${id.substring(id.length - 6)}' : id;
    }
    return 'Tab ${index + 1}';
  }
}
