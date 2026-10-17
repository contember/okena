/**
 * WorkspaceScreen.tsx — the connected workspace.
 *
 * Port of `mobile/lib/src/screens/workspace_screen.dart`. Composes:
 *   - an app bar: a drawer-toggle (☰), the selected project name (tap to switch
 *     when there are several), a small connection-quality dot, a fullscreen
 *     toggle, and a "＋" terminal-actions menu (new / split V / split H /
 *     minimize). StatusIndicator is NOT imported (owned by another agent) — the
 *     dot is inline.
 *   - the layout area: the parsed project layout tree rendered by
 *     {@link LayoutRenderer}; when fullscreen, only the fullscreen terminal is
 *     shown via a single {@link TerminalPane}. Empty/no-terminal states match
 *     the Dart screen.
 *   - the {@link KeyToolbar} pinned above the soft keyboard (rendered inside a
 *     `KeyboardAvoidingView` so it sits on top of the keyboard).
 *   - the slide-in {@link ProjectDrawer}.
 *
 * Fonts are loaded here with Skia's `useFont` (see README) and threaded down to
 * every terminal pane; the layout area guards on fonts being loaded.
 *
 * State comes from the stores; polling lifecycle is owned by App.tsx (this
 * screen only reads + dispatches actions).
 */

import React, { useMemo, useRef, useState } from 'react';
import {
  View,
  Text,
  Pressable,
  KeyboardAvoidingView,
  Platform,
  Modal,
  ScrollView,
  StyleSheet,
  TextInput,
} from 'react-native';
import { useFont } from '@shopify/react-native-skia';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import { useWorkspaceStore, useConnectionStore } from '../state';
import {
  buildProjectList,
  filterProjectList,
  flattenProjectList,
  parseLayout,
  projectDisplayName,
} from '../models';
import { getOkenaNative, type OkenaNative } from '../native/okena';
import { OkenaColors, TerminalTheme } from '../theme';
import { LayoutRenderer } from '../components/LayoutRenderer';
import { TerminalPane, type TerminalPaneHandle } from '../components/TerminalPane';
import { KeyToolbar, KeyModifiers } from '../components/KeyToolbar';
import { ProjectDrawer } from '../components/ProjectDrawer';
import type { TerminalFonts } from '../components/TerminalView';

const native: OkenaNative = (() => {
  try {
    return getOkenaNative();
  } catch {
    // Native not wired up (off-device). The screen still renders chrome; the
    // panes guard their native calls. Throwing here would crash the whole app.
    return undefined as unknown as OkenaNative;
  }
})();

export const WorkspaceScreen: React.FC = () => {
  const insets = useSafeAreaInsets();
  // ── fonts (Skia useFont; null until loaded) ────────────────────────────────
  const fontSize = TerminalTheme.defaultFontSize;
  const regular = useFont(require('../../assets/JetBrainsMono-Regular.ttf'), fontSize);
  const bold = useFont(require('../../assets/JetBrainsMono-Bold.ttf'), fontSize);
  const italic = useFont(require('../../assets/JetBrainsMono-Italic.ttf'), fontSize);
  const boldItalic = useFont(require('../../assets/JetBrainsMono-BoldItalic.ttf'), fontSize);

  const fonts: TerminalFonts | null = useMemo(
    () =>
      regular
        ? {
            regular,
            bold: bold ?? undefined,
            italic: italic ?? undefined,
            boldItalic: boldItalic ?? undefined,
          }
        : null,
    [regular, bold, italic, boldItalic],
  );

  // ── shared key-modifier store (toolbar + soft keyboard) ─────────────────────
  const modifiers = useRef(new KeyModifiers()).current;
  const paneRef = useRef<TerminalPaneHandle>(null);

  // ── stores ──────────────────────────────────────────────────────────────────
  const projects = useWorkspaceStore((s) => s.projects);
  const folders = useWorkspaceStore((s) => s.folders);
  const projectOrder = useWorkspaceStore((s) => s.projectOrder);
  const selectedProjectId = useWorkspaceStore((s) => s.selectedProjectId);
  const selectedTerminalId = useWorkspaceStore((s) => s.selectedTerminalId);
  const fullscreenTerminal = useWorkspaceStore((s) => s.fullscreenTerminal);
  const layoutJson = useWorkspaceStore((s) => s.projectLayoutJson);
  const secondsSinceActivity = useWorkspaceStore((s) => s.secondsSinceActivity);
  const selectProject = useWorkspaceStore((s) => s.selectProject);
  const selectTerminal = useWorkspaceStore((s) => s.selectTerminal);
  // Recompute the selected project from the live id (cheap selector helper).
  const project = useMemo(() => {
    if (selectedProjectId === null) return projects[0] ?? null;
    return projects.find((p) => p.id === selectedProjectId) ?? projects[0] ?? null;
  }, [projects, selectedProjectId]);

  const connId = useConnectionStore((s) => s.connId);

  const [drawerOpen, setDrawerOpen] = useState(false);
  const [switcherOpen, setSwitcherOpen] = useState(false);
  const [actionsOpen, setActionsOpen] = useState(false);
  const [switcherQuery, setSwitcherQuery] = useState('');

  // Switcher rows in desktop sidebar order, filtered by the search field.
  const projectList = useMemo(
    () => buildProjectList(projects, folders, projectOrder),
    [projects, folders, projectOrder],
  );
  const switcherRows = useMemo(
    () => flattenProjectList(filterProjectList(projectList, switcherQuery)),
    [projectList, switcherQuery],
  );
  const closeSwitcher = () => {
    setSwitcherOpen(false);
    setSwitcherQuery('');
  };

  // Layout tree for the selected project. Keyed on the authoritative JSON, so
  // tab switches, minimizes and split changes re-render even though they leave
  // the terminal id set untouched.
  const layoutNode = useMemo(
    () => (layoutJson ? parseLayout(layoutJson) : null),
    [layoutJson],
  );

  const dotColor =
    secondsSinceActivity < 3
      ? OkenaColors.success
      : secondsSinceActivity < 10
        ? OkenaColors.warning
        : OkenaColors.error;

  const projectName = project ? projectDisplayName(project) : 'No Project';

  // ── terminal actions ──────────────────────────────────────────────────────
  const onNewTerminal = () => {
    if (connId && project) void native?.createTerminal(connId, project.id);
  };
  const onSplit = (direction: 'vertical' | 'horizontal') => {
    if (connId && project) void native?.splitTerminal(connId, project.id, [], direction);
  };
  const onMinimize = () => {
    if (connId && project && selectedTerminalId) {
      void native?.toggleMinimized(connId, project.id, selectedTerminalId);
    }
  };
  const onToggleFullscreen = () => {
    if (!connId || !project) return;
    if (fullscreenTerminal) {
      void native?.setFullscreen(connId, project.id, undefined);
    } else if (selectedTerminalId) {
      void native?.setFullscreen(connId, project.id, selectedTerminalId);
    }
  };

  // ── body ──────────────────────────────────────────────────────────────────
  const renderBody = () => {
    if (!connId || !project) {
      return (
        <Centered>
          <Text style={styles.dim}>No project selected</Text>
        </Centered>
      );
    }
    if (!fonts) {
      return (
        <Centered>
          <Text style={styles.dim}>Loading fonts…</Text>
        </Centered>
      );
    }
    // Fullscreen: just the one terminal.
    if (fullscreenTerminal && fullscreenTerminal.projectId === project.id) {
      return (
        <TerminalPane
          ref={paneRef}
          connId={connId}
          terminalId={fullscreenTerminal.terminalId}
          fonts={fonts}
          modifiers={modifiers}
          onSelect={selectTerminal}
          selected={selectedTerminalId === fullscreenTerminal.terminalId}
          native={native}
        />
      );
    }
    if (project.terminalIds.length === 0) {
      return (
        <Centered>
          <Text style={styles.dim}>No terminals</Text>
          <Pressable style={styles.primaryBtn} onPress={onNewTerminal}>
            <Text style={styles.primaryBtnText}>New Terminal</Text>
          </Pressable>
        </Centered>
      );
    }
    if (layoutNode) {
      return (
        <LayoutRenderer
          connId={connId}
          projectId={project.id}
          node={layoutNode}
          fonts={fonts}
          modifiers={modifiers}
          terminalIds={project.terminalIds}
          paneRef={paneRef}
          focusTerminalId={selectedTerminalId}
          onSelectTerminal={selectTerminal}
          native={native}
        />
      );
    }
    // Fallback: render the selected (or first) terminal directly.
    const tid = selectedTerminalId ?? project.terminalIds[0]!;
    return (
      <TerminalPane
        ref={paneRef}
        connId={connId}
        terminalId={tid}
        fonts={fonts}
        modifiers={modifiers}
        onSelect={selectTerminal}
        selected={selectedTerminalId === tid}
        native={native}
      />
    );
  };

  const showToolbar = connId !== null && project !== null && selectedTerminalId !== null;

  return (
    <View style={styles.root}>
      {/* App bar */}
      <View style={styles.appBar}>
        <Pressable hitSlop={8} style={styles.appBarBtn} onPress={() => setDrawerOpen(true)}>
          <Text style={styles.appBarIcon}>{'☰'}</Text>
        </Pressable>
        <Pressable
          style={styles.titleWrap}
          disabled={projects.length <= 1}
          onPress={() => setSwitcherOpen(true)}
        >
          <Text style={styles.title} numberOfLines={1}>
            {projectName}
          </Text>
          {projects.length > 1 ? <Text style={styles.titleCaret}>{' ▾'}</Text> : null}
        </Pressable>
        <View style={styles.flexSpacer} />
        {connId ? <View style={[styles.dot, { backgroundColor: dotColor }]} /> : null}
        {connId && project && selectedTerminalId ? (
          <Pressable hitSlop={8} style={styles.appBarBtn} onPress={onToggleFullscreen}>
            <Text style={styles.appBarIcon}>{fullscreenTerminal ? '🗗' : '⛶'}</Text>
          </Pressable>
        ) : null}
        {connId && project ? (
          <Pressable hitSlop={8} style={styles.appBarBtn} onPress={() => setActionsOpen(true)}>
            <Text style={styles.appBarIcon}>{'＋'}</Text>
          </Pressable>
        ) : null}
      </View>

      {/* Body + key toolbar (toolbar rides above the keyboard) */}
      <KeyboardAvoidingView
        style={styles.flex}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
        // The keyboard frame is in screen coordinates, but this view's frame is
        // relative to the safe-area container; offset by the top inset.
        keyboardVerticalOffset={insets.top}
      >
        <View style={styles.flex}>{renderBody()}</View>
        {showToolbar && connId ? (
          <KeyToolbar
            connId={connId}
            terminalId={selectedTerminalId}
            modifiers={modifiers}
            native={native}
            onHideKeyboard={() => paneRef.current?.blur()}
            onShowKeyboard={() => paneRef.current?.focus()}
          />
        ) : null}
      </KeyboardAvoidingView>

      {/* Drawer */}
      <ProjectDrawer open={drawerOpen} onClose={() => setDrawerOpen(false)} native={native} />

      {/* Project switcher */}
      <Modal
        visible={switcherOpen}
        transparent
        animationType="fade"
        onRequestClose={closeSwitcher}
      >
        <Pressable style={styles.menuBackdrop} onPress={closeSwitcher}>
          {/* Inner Pressable swallows taps so they don't close the menu. */}
          <Pressable style={styles.menu} onPress={() => {}}>
            <TextInput
              style={styles.menuSearch}
              placeholder="Search projects"
              placeholderTextColor={OkenaColors.textTertiary}
              value={switcherQuery}
              onChangeText={setSwitcherQuery}
              autoCapitalize="none"
              autoCorrect={false}
              spellCheck={false}
              clearButtonMode="while-editing"
              returnKeyType="search"
              keyboardAppearance="dark"
            />
            <ScrollView
              keyboardShouldPersistTaps="handled"
              keyboardDismissMode="on-drag"
              automaticallyAdjustKeyboardInsets
            >
              {switcherRows.length === 0 ? (
                <Text style={[styles.menuItem, styles.dim]}>No matching projects</Text>
              ) : null}
              {switcherRows.map((row) => {
                if (row.kind === 'folder') {
                  return (
                    <Text key={`folder-${row.folder.id}`} style={styles.menuFolder} numberOfLines={1}>
                      {row.folder.name}
                    </Text>
                  );
                }
                const { project: p, depth } = row;
                const isSel = p.id === project?.id;
                return (
                  <Pressable
                    key={p.id}
                    style={[styles.menuItem, { paddingLeft: 16 + depth * 12 }]}
                    onPress={() => {
                      selectProject(p.id);
                      closeSwitcher();
                    }}
                  >
                    <Text style={[styles.menuItemText, isSel && styles.menuItemSelected]} numberOfLines={1}>
                      {projectDisplayName(p)}
                    </Text>
                    {p.gitBranch && !p.parentProjectId ? (
                      <Text style={styles.menuBranch} numberOfLines={1}>
                        {`⎇ ${p.gitBranch}`}
                      </Text>
                    ) : null}
                  </Pressable>
                );
              })}
            </ScrollView>
          </Pressable>
        </Pressable>
      </Modal>

      {/* Terminal actions menu */}
      <Modal
        visible={actionsOpen}
        transparent
        animationType="slide"
        onRequestClose={() => setActionsOpen(false)}
      >
        <Pressable style={styles.sheetBackdrop} onPress={() => setActionsOpen(false)} />
        <View style={styles.sheet}>
          <ActionItem
            label="New Terminal"
            onPress={() => {
              setActionsOpen(false);
              onNewTerminal();
            }}
          />
          {selectedTerminalId ? (
            <>
              <ActionItem
                label="Split Vertical"
                onPress={() => {
                  setActionsOpen(false);
                  onSplit('vertical');
                }}
              />
              <ActionItem
                label="Split Horizontal"
                onPress={() => {
                  setActionsOpen(false);
                  onSplit('horizontal');
                }}
              />
              <ActionItem
                label="Minimize"
                onPress={() => {
                  setActionsOpen(false);
                  onMinimize();
                }}
              />
            </>
          ) : null}
        </View>
      </Modal>
    </View>
  );
};

const Centered: React.FC<{ children: React.ReactNode }> = ({ children }) => (
  <View style={styles.centered}>{children}</View>
);

const ActionItem: React.FC<{ label: string; onPress: () => void }> = ({ label, onPress }) => (
  <Pressable style={styles.sheetItem} onPress={onPress}>
    <Text style={styles.sheetItemText}>{label}</Text>
  </Pressable>
);

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: OkenaColors.background },
  flex: { flex: 1 },
  flexSpacer: { flex: 1 },
  appBar: {
    height: 52,
    paddingHorizontal: 8,
    flexDirection: 'row',
    alignItems: 'center',
    backgroundColor: OkenaColors.surface,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
  appBarBtn: { padding: 8 },
  appBarIcon: { color: OkenaColors.textPrimary, fontSize: 18 },
  titleWrap: { flexDirection: 'row', alignItems: 'center', flexShrink: 1 },
  title: { color: OkenaColors.textPrimary, fontSize: 17, fontWeight: '600', flexShrink: 1 },
  titleCaret: { color: OkenaColors.textSecondary, fontSize: 14 },
  dot: { width: 8, height: 8, borderRadius: 4, marginHorizontal: 6 },
  centered: { flex: 1, alignItems: 'center', justifyContent: 'center', padding: 24 },
  dim: { color: OkenaColors.textTertiary, fontSize: 14 },
  primaryBtn: {
    marginTop: 16,
    paddingHorizontal: 20,
    paddingVertical: 10,
    borderRadius: 8,
    backgroundColor: OkenaColors.accent,
  },
  primaryBtnText: { color: '#ffffff', fontSize: 14, fontWeight: '600' },
  // project switcher menu
  menuBackdrop: { flex: 1, backgroundColor: 'rgba(0,0,0,0.3)', paddingTop: 96, paddingLeft: 48 },
  menu: {
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 10,
    maxHeight: 360,
    width: 260,
    paddingVertical: 4,
  },
  menuItem: { paddingHorizontal: 16, paddingVertical: 10 },
  menuSearch: {
    marginHorizontal: 8,
    marginVertical: 4,
    paddingHorizontal: 10,
    paddingVertical: 8,
    borderRadius: 8,
    backgroundColor: OkenaColors.surface,
    color: OkenaColors.textPrimary,
    fontSize: 14,
  },
  menuFolder: {
    paddingHorizontal: 16,
    paddingTop: 10,
    paddingBottom: 2,
    color: OkenaColors.textSecondary,
    fontSize: 11,
    fontWeight: '600',
    letterSpacing: 0.5,
  },
  menuItemText: { color: OkenaColors.textPrimary, fontSize: 14 },
  menuItemSelected: { color: OkenaColors.accent, fontWeight: '700' },
  menuBranch: { color: OkenaColors.textTertiary, fontSize: 11, marginTop: 2 },
  // action sheet
  sheetBackdrop: { flex: 1, backgroundColor: 'rgba(0,0,0,0.4)' },
  sheet: {
    backgroundColor: OkenaColors.surfaceElevated,
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
    padding: 8,
    paddingBottom: 32,
  },
  sheetItem: { paddingVertical: 14, paddingHorizontal: 12 },
  sheetItemText: { color: OkenaColors.textPrimary, fontSize: 15 },
});

export default WorkspaceScreen;
