sealed class LayoutNode {
  const LayoutNode();

  factory LayoutNode.fromJson(Map<String, dynamic> json) {
    final type = json['type'] as String;
    return switch (type) {
      'terminal' => TerminalNode(
          terminalId: json['terminal_id'] as String?,
          minimized: json['minimized'] as bool? ?? false,
          detached: json['detached'] as bool? ?? false,
        ),
      'split' => SplitNode(
          direction: json['direction'] as String? ?? 'horizontal',
          sizes: (json['sizes'] as List?)
                  ?.map((e) => (e as num).toDouble())
                  .toList() ??
              [],
          children: (json['children'] as List?)
                  ?.map((e) => LayoutNode.fromJson(e as Map<String, dynamic>))
                  .toList() ??
              [],
        ),
      'tabs' => TabsNode(
          activeTab: json['active_tab'] as int? ?? 0,
          children: (json['children'] as List?)
                  ?.map((e) => LayoutNode.fromJson(e as Map<String, dynamic>))
                  .toList() ??
              [],
        ),
      _ => TerminalNode(terminalId: null),
    };
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
  final String direction; // "horizontal" or "vertical"
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
