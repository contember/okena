import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/src/models/layout_node.dart';

void main() {
  group('LayoutNode.fromJson', () {
    test('parses terminal node', () {
      final node = LayoutNode.fromJson({
        'type': 'terminal',
        'terminal_id': 't1',
        'minimized': false,
        'detached': false,
      });

      expect(node, isA<TerminalNode>());
      final t = node as TerminalNode;
      expect(t.terminalId, 't1');
      expect(t.minimized, isFalse);
      expect(t.detached, isFalse);
    });

    test('parses terminal node with null id', () {
      final node = LayoutNode.fromJson({
        'type': 'terminal',
        'terminal_id': null,
        'minimized': true,
        'detached': true,
      });

      expect(node, isA<TerminalNode>());
      expect((node as TerminalNode).terminalId, isNull);
    });

    test('parses split node', () {
      final node = LayoutNode.fromJson({
        'type': 'split',
        'direction': 'horizontal',
        'sizes': [50.0, 50.0],
        'children': [
          {'type': 'terminal', 'terminal_id': 't1', 'minimized': false, 'detached': false},
          {'type': 'terminal', 'terminal_id': 't2', 'minimized': false, 'detached': false},
        ],
      });

      expect(node, isA<SplitNode>());
      final s = node as SplitNode;
      expect(s.direction, 'horizontal');
      expect(s.sizes, [50.0, 50.0]);
      expect(s.children.length, 2);
      expect((s.children[0] as TerminalNode).terminalId, 't1');
      expect((s.children[1] as TerminalNode).terminalId, 't2');
    });

    test('parses tabs node', () {
      final node = LayoutNode.fromJson({
        'type': 'tabs',
        'active_tab': 1,
        'children': [
          {'type': 'terminal', 'terminal_id': 'a', 'minimized': false, 'detached': false},
          {'type': 'terminal', 'terminal_id': 'b', 'minimized': false, 'detached': false},
        ],
      });

      expect(node, isA<TabsNode>());
      final t = node as TabsNode;
      expect(t.activeTab, 1);
      expect(t.children.length, 2);
    });

    test('parses nested split with tabs', () {
      final node = LayoutNode.fromJson({
        'type': 'split',
        'direction': 'vertical',
        'sizes': [30.0, 70.0],
        'children': [
          {'type': 'terminal', 'terminal_id': 't1', 'minimized': false, 'detached': false},
          {
            'type': 'tabs',
            'active_tab': 0,
            'children': [
              {'type': 'terminal', 'terminal_id': 't2', 'minimized': false, 'detached': false},
              {'type': 'terminal', 'terminal_id': 't3', 'minimized': false, 'detached': false},
            ],
          },
        ],
      });

      expect(node, isA<SplitNode>());
      final s = node as SplitNode;
      expect(s.children[0], isA<TerminalNode>());
      expect(s.children[1], isA<TabsNode>());
      final tabs = s.children[1] as TabsNode;
      expect(tabs.children.length, 2);
    });

    test('unknown type falls back to empty terminal', () {
      final node = LayoutNode.fromJson({
        'type': 'unknown_future_type',
      });

      expect(node, isA<TerminalNode>());
      expect((node as TerminalNode).terminalId, isNull);
    });

    test('handles missing optional fields with defaults', () {
      final node = LayoutNode.fromJson({
        'type': 'split',
      });

      expect(node, isA<SplitNode>());
      final s = node as SplitNode;
      expect(s.direction, 'horizontal');
      expect(s.sizes, isEmpty);
      expect(s.children, isEmpty);
    });
  });
}
