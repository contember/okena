import 'dart:convert';

/// Sealed layout node hierarchy matching ApiLayoutNode from the server.
sealed class LayoutNode {
  const LayoutNode();

  static LayoutNode? fromJson(String jsonStr) {
    try {
      final map = json.decode(jsonStr) as Map<String, dynamic>;
      return _parse(map);
    } catch (_) {
      return null;
    }
  }

  static LayoutNode? _parse(Map<String, dynamic> map) {
    final type = map['type'] as String?;
    switch (type) {
      case 'terminal':
        return TerminalNode(
          terminalId: map['terminal_id'] as String?,
          minimized: map['minimized'] as bool? ?? false,
          detached: map['detached'] as bool? ?? false,
        );
      case 'split':
        final children = (map['children'] as List?)
                ?.map((c) => _parse(c as Map<String, dynamic>))
                .whereType<LayoutNode>()
                .toList() ??
            [];
        final sizes = (map['sizes'] as List?)
                ?.map((s) => (s as num).toDouble())
                .toList() ??
            [];
        return SplitNode(
          direction: map['direction'] == 'vertical'
              ? SplitDirection.vertical
              : SplitDirection.horizontal,
          sizes: sizes,
          children: children,
        );
      case 'tabs':
        final children = (map['children'] as List?)
                ?.map((c) => _parse(c as Map<String, dynamic>))
                .whereType<LayoutNode>()
                .toList() ??
            [];
        return TabsNode(
          activeTab: map['active_tab'] as int? ?? 0,
          children: children,
        );
      default:
        return null;
    }
  }
}

class TerminalNode extends LayoutNode {
  final String? terminalId;
  final bool minimized;
  final bool detached;

  const TerminalNode({
    this.terminalId,
    this.minimized = false,
    this.detached = false,
  });
}

class SplitNode extends LayoutNode {
  final SplitDirection direction;
  final List<double> sizes;
  final List<LayoutNode> children;

  const SplitNode({
    required this.direction,
    required this.sizes,
    required this.children,
  });
}

class TabsNode extends LayoutNode {
  final int activeTab;
  final List<LayoutNode> children;

  const TabsNode({
    required this.activeTab,
    required this.children,
  });
}

enum SplitDirection { horizontal, vertical }
