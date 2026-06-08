/**
 * LayoutRenderer.tsx — recursive project-layout tree renderer.
 *
 * Port of `mobile/lib/src/widgets/layout_renderer.dart`. Walks a parsed
 * {@link LayoutNode} tree (from `parseLayout(getProjectLayoutJson())`) and
 * renders:
 *   - `TerminalNode`  → a {@link TerminalPane} (the renderer container), or a
 *     minimized placeholder bar when `minimized`.
 *   - `SplitNode`     → a flex row (horizontal) / column (vertical) sized by the
 *     node's `sizes` (used as flex weights). **Portrait rotation**: a horizontal
 *     split is forced to render vertically in portrait orientation, matching the
 *     Dart `isVertical = vertical || (horizontal && isPortrait)` rule.
 *   - `TabsNode`      → a horizontal tab bar + only the active child mounted.
 *
 * Each child carries its `path` (the list of child indices from the root), which
 * the tab / minimize / split actions pass back to the native module.
 *
 * Presentational + injected `native` (defaults to `getOkenaNative()`), and the
 * shared `modifiers` store + `fonts` are threaded down to every `TerminalPane`,
 * mirroring `TerminalView`/`TerminalPane`.
 *
 * NOTE: unlike the Flutter version this does NOT implement draggable split
 * dividers (no pan-resize on mobile here) — split sizes come straight from the
 * server layout. Tab switching and minimize toggling are wired.
 */

import React from 'react';
import {
  View,
  Text,
  Pressable,
  ScrollView,
  StyleSheet,
  useWindowDimensions,
} from 'react-native';

import type { LayoutNode } from '../models';
import type { OkenaNative } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import { OkenaColors } from '../theme';
import { TerminalPane, type TerminalPaneHandle } from './TerminalPane';
import type { TerminalFonts } from './TerminalView';
import type { KeyModifiers } from './KeyToolbar';

export interface LayoutRendererProps {
  connId: string;
  projectId: string;
  node: LayoutNode;
  fonts: TerminalFonts;
  modifiers: KeyModifiers;
  /** All terminal ids in the project (used only for the empty/fallback case). */
  terminalIds?: string[];
  /** Ref to the currently-focused terminal pane (for keyboard focus/blur). */
  paneRef?: React.Ref<TerminalPaneHandle>;
  /** The terminal id whose pane should receive {@link paneRef}. */
  focusTerminalId?: string | null;
  native?: OkenaNative;
}

export const LayoutRenderer: React.FC<LayoutRendererProps> = ({
  connId,
  projectId,
  node,
  fonts,
  modifiers,
  paneRef,
  focusTerminalId,
  native = getOkenaNative(),
}) => {
  const { width, height } = useWindowDimensions();
  const isPortrait = height >= width;

  return (
    <NodeView
      connId={connId}
      projectId={projectId}
      node={node}
      path={[]}
      fonts={fonts}
      modifiers={modifiers}
      isPortrait={isPortrait}
      paneRef={paneRef}
      focusTerminalId={focusTerminalId ?? null}
      native={native}
    />
  );
};

// ── Recursive node renderer ────────────────────────────────────────────────

interface NodeViewProps {
  connId: string;
  projectId: string;
  node: LayoutNode;
  path: number[];
  fonts: TerminalFonts;
  modifiers: KeyModifiers;
  isPortrait: boolean;
  paneRef?: React.Ref<TerminalPaneHandle>;
  focusTerminalId: string | null;
  native: OkenaNative;
}

const NodeView: React.FC<NodeViewProps> = (props) => {
  const { node } = props;
  switch (node.type) {
    case 'terminal':
      return <TerminalLeaf {...props} node={node} />;
    case 'split':
      return <SplitView {...props} node={node} />;
    case 'tabs':
      return <TabsView {...props} node={node} />;
  }
};

// ── Terminal leaf ────────────────────────────────────────────────────────────

const TerminalLeaf: React.FC<NodeViewProps & { node: Extract<LayoutNode, { type: 'terminal' }> }> = ({
  connId,
  projectId,
  node,
  fonts,
  modifiers,
  paneRef,
  focusTerminalId,
  native,
}) => {
  const { terminalId, minimized } = node;

  if (!terminalId) {
    return (
      <View style={styles.placeholder}>
        <Text style={styles.placeholderText}>Empty terminal</Text>
      </View>
    );
  }

  if (minimized) {
    const short =
      terminalId.length > 8 ? `...${terminalId.slice(-8)}` : terminalId;
    return (
      <Pressable
        style={styles.minimized}
        onPress={() => {
          void native.toggleMinimized(connId, projectId, terminalId);
        }}
      >
        <Text style={styles.minimizedIcon}>{'▸'}</Text>
        <Text style={styles.minimizedText}>{short}</Text>
        <View style={styles.flexSpacer} />
        <Text style={styles.minimizedChevron}>{'⌄'}</Text>
      </Pressable>
    );
  }

  // Wire the imperative pane ref only to the focused terminal.
  const refForThis = focusTerminalId === terminalId ? paneRef : undefined;

  return (
    <View style={styles.flex}>
      <TerminalPane
        ref={refForThis ?? null}
        connId={connId}
        terminalId={terminalId}
        fonts={fonts}
        modifiers={modifiers}
        native={native}
      />
    </View>
  );
};

// ── Split ───────────────────────────────────────────────────────────────────

const SplitView: React.FC<NodeViewProps & { node: Extract<LayoutNode, { type: 'split' }> }> = ({
  node,
  path,
  isPortrait,
  ...rest
}) => {
  const { direction, sizes, children } = node;
  if (children.length === 0) return <View style={styles.flex} />;

  // Portrait rotation: force horizontal splits to render vertically.
  const isVertical =
    direction === 'vertical' || (direction === 'horizontal' && isPortrait);

  return (
    <View style={[styles.flex, isVertical ? styles.column : styles.row]}>
      {children.map((child, i) => {
        const flex =
          i < sizes.length ? Math.min(Math.max(Math.round(sizes[i] ?? 1), 1), 1000) : 1;
        return (
          <View key={i} style={{ flex }}>
            <NodeView
              {...rest}
              node={child}
              path={[...path, i]}
              isPortrait={isPortrait}
            />
          </View>
        );
      })}
    </View>
  );
};

// ── Tabs ──────────────────────────────────────────────────────────────────

const TabsView: React.FC<NodeViewProps & { node: Extract<LayoutNode, { type: 'tabs' }> }> = ({
  connId,
  projectId,
  node,
  path,
  native,
  isPortrait,
  ...rest
}) => {
  const { children } = node;
  if (children.length === 0) return <View style={styles.flex} />;

  const activeTab = Math.min(Math.max(node.activeTab, 0), children.length - 1);
  const activeChild = children[activeTab]!;

  const tabLabel = (child: LayoutNode, index: number): string => {
    if (child.type === 'terminal' && child.terminalId) {
      const id = child.terminalId;
      return id.length > 6 ? `...${id.slice(-6)}` : id;
    }
    return `Tab ${index + 1}`;
  };

  return (
    <View style={styles.flex}>
      <View style={styles.tabBar}>
        <ScrollView
          horizontal
          showsHorizontalScrollIndicator={false}
          style={styles.flex}
        >
          {children.map((child, i) => {
            const isActive = i === activeTab;
            return (
              <Pressable
                key={i}
                style={[styles.tab, isActive && styles.tabActive]}
                onPress={() => {
                  if (i !== activeTab) {
                    void native.setActiveTab(connId, projectId, path, i);
                  }
                }}
              >
                <Text style={[styles.tabText, isActive && styles.tabTextActive]}>
                  {tabLabel(child, i)}
                </Text>
              </Pressable>
            );
          })}
        </ScrollView>
        <Pressable
          style={styles.tabAdd}
          onPress={() => {
            void native.addTab(connId, projectId, path, true);
          }}
        >
          <Text style={styles.tabAddText}>+</Text>
        </Pressable>
      </View>
      <View style={styles.flex}>
        <NodeView
          {...rest}
          connId={connId}
          projectId={projectId}
          node={activeChild}
          path={[...path, activeTab]}
          native={native}
          isPortrait={isPortrait}
        />
      </View>
    </View>
  );
};

const styles = StyleSheet.create({
  flex: { flex: 1 },
  row: { flexDirection: 'row' },
  column: { flexDirection: 'column' },
  flexSpacer: { flex: 1 },
  placeholder: { flex: 1, alignItems: 'center', justifyContent: 'center' },
  placeholderText: { color: OkenaColors.textTertiary },
  minimized: {
    height: 36,
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 12,
    backgroundColor: OkenaColors.surfaceElevated,
  },
  minimizedIcon: { color: OkenaColors.textSecondary, fontSize: 12, marginRight: 8 },
  minimizedText: {
    color: OkenaColors.textSecondary,
    fontSize: 12,
    fontFamily: 'JetBrainsMono',
  },
  minimizedChevron: { color: OkenaColors.textTertiary, fontSize: 14 },
  tabBar: {
    height: 32,
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: OkenaColors.surfaceElevated,
  },
  tab: {
    paddingHorizontal: 12,
    paddingVertical: 6,
    borderBottomWidth: 2,
    borderBottomColor: 'transparent',
    justifyContent: 'center',
  },
  tabActive: { borderBottomColor: OkenaColors.accent },
  tabText: { color: OkenaColors.textSecondary, fontSize: 12 },
  tabTextActive: { color: OkenaColors.textPrimary },
  tabAdd: { paddingHorizontal: 8, justifyContent: 'center' },
  tabAddText: { color: OkenaColors.textTertiary, fontSize: 16 },
});

export default LayoutRenderer;
