import 'dart:convert';

import 'package:flutter/material.dart';

import '../../src/rust/api/state.dart' as state_ffi;
import '../models/layout_node.dart';
import 'terminal_view.dart';

class LayoutRenderer extends StatelessWidget {
  final String connId;
  final String projectId;
  final List<String> terminalIds;

  const LayoutRenderer({
    super.key,
    required this.connId,
    required this.projectId,
    required this.terminalIds,
  });

  @override
  Widget build(BuildContext context) {
    final json = state_ffi.getProjectLayoutJson(
      connId: connId,
      projectId: projectId,
    );

    if (json != null) {
      try {
        final node =
            LayoutNode.fromJson(jsonDecode(json) as Map<String, dynamic>);
        return _buildNode(context, node);
      } catch (_) {
        // Fall through to fallback
      }
    }

    // Fallback: show first terminal
    if (terminalIds.isEmpty) {
      return const Center(
        child: Text(
          'No terminals',
          style: TextStyle(color: Colors.grey),
        ),
      );
    }
    return TerminalView(connId: connId, terminalId: terminalIds.first);
  }

  Widget _buildNode(BuildContext context, LayoutNode node) {
    return switch (node) {
      TerminalNode(:final terminalId) => terminalId != null
          ? TerminalView(connId: connId, terminalId: terminalId)
          : const Center(
              child:
                  Text('Empty terminal', style: TextStyle(color: Colors.grey)),
            ),
      SplitNode(:final direction, :final sizes, :final children) =>
        _buildSplit(context, direction, sizes, children),
      TabsNode(:final activeTab, :final children) =>
        _buildTabs(context, activeTab, children),
    };
  }

  Widget _buildSplit(
    BuildContext context,
    String direction,
    List<double> sizes,
    List<LayoutNode> children,
  ) {
    if (children.isEmpty) {
      return const SizedBox.shrink();
    }

    // In portrait mode, force horizontal splits to vertical
    final isPortrait =
        MediaQuery.of(context).orientation == Orientation.portrait;
    final isVertical =
        direction == 'vertical' || (direction == 'horizontal' && isPortrait);

    final flexChildren = <Widget>[];
    for (int i = 0; i < children.length; i++) {
      final flex = i < sizes.length ? sizes[i].round().clamp(1, 1000) : 1;
      if (i > 0) {
        flexChildren.add(
          isVertical
              ? const Divider(
                  height: 2, thickness: 2, color: Color(0xFF363650))
              : const VerticalDivider(
                  width: 2, thickness: 2, color: Color(0xFF363650)),
        );
      }
      flexChildren.add(
        Expanded(flex: flex, child: _buildNode(context, children[i])),
      );
    }

    return Flex(
      direction: isVertical ? Axis.vertical : Axis.horizontal,
      children: flexChildren,
    );
  }

  Widget _buildTabs(
    BuildContext context,
    int activeTab,
    List<LayoutNode> children,
  ) {
    if (children.isEmpty) {
      return const SizedBox.shrink();
    }

    return _TabsWidget(
      initialTab: activeTab.clamp(0, children.length - 1),
      children: children,
      builder: (node) => _buildNode(context, node),
    );
  }
}

class _TabsWidget extends StatefulWidget {
  final int initialTab;
  final List<LayoutNode> children;
  final Widget Function(LayoutNode) builder;

  const _TabsWidget({
    required this.initialTab,
    required this.children,
    required this.builder,
  });

  @override
  State<_TabsWidget> createState() => _TabsWidgetState();
}

class _TabsWidgetState extends State<_TabsWidget> {
  late int _activeTab;

  @override
  void initState() {
    super.initState();
    _activeTab = widget.initialTab;
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      children: [
        Container(
          height: 32,
          color: const Color(0xFF2A2A3E),
          child: Row(
            children: [
              for (int i = 0; i < widget.children.length; i++)
                GestureDetector(
                  onTap: () => setState(() => _activeTab = i),
                  child: Container(
                    padding:
                        const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
                    decoration: BoxDecoration(
                      border: Border(
                        bottom: BorderSide(
                          color: i == _activeTab
                              ? Colors.blue.shade300
                              : Colors.transparent,
                          width: 2,
                        ),
                      ),
                    ),
                    child: Text(
                      _tabLabel(widget.children[i], i),
                      style: TextStyle(
                        color: i == _activeTab ? Colors.white : Colors.white54,
                        fontSize: 12,
                      ),
                    ),
                  ),
                ),
            ],
          ),
        ),
        Expanded(
          child: widget.builder(widget.children[_activeTab]),
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
