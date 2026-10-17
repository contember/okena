/**
 * WorkspaceScreen.tsx — the connected workspace.
 *
 * Port of `mobile/lib/src/screens/workspace_screen.dart`. Composes:
 *   - an app bar: a drawer-toggle (☰), the selected project name (tap to switch
 *     when there are several), the agent usage rings (tap for the
 *     {@link UsageSheet}), a small connection-quality dot, and a "＋"
 *     terminal-actions menu (new / split V / split H). StatusIndicator is NOT
 *     imported (owned by another agent) — the dot is inline.
 *   - when the project has several terminals, a {@link TerminalTabs} chip row
 *     to switch between them.
 *   - the terminal area: ONE {@link TerminalPane}, full size. With the "Open
 *     keyboard with terminal" setting it focuses itself when shown, but never
 *     while the drawer, the switcher or the actions sheet is open. The desktop split
 *     layout is not reproduced on the phone (it made every pane too small); the
 *     server layout is left untouched. No fullscreen toggle either: with one
 *     pane on screen it would only change the desktop's fullscreen state.
 *     Empty/no-terminal states match the Dart screen.
 *   - the {@link KeyToolbar} pinned above the soft keyboard (rendered inside a
 *     `KeyboardAvoidingView` so it sits on top of the keyboard).
 *   - the slide-in {@link ProjectDrawer}.
 *   - the project switcher (tap the project name). In the Activity order the
 *     current project comes first and the previously viewed one right under
 *     it, so two projects toggle with one tap; the rest follow in activity
 *     order with tier headers, frozen while the switcher is open. A search
 *     narrows to matching projects in activity order. The Manual order keeps
 *     the desktop's manual list with folders.
 *
 * Fonts are loaded here with Skia's `useFont` (see README) and threaded down to
 * every terminal pane; the layout area guards on fonts being loaded.
 *
 * State comes from the stores; polling lifecycle is owned by App.tsx (this
 * screen only reads + dispatches actions).
 */

import React, { useEffect, useMemo, useRef, useState } from 'react';
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
  findTerminalPath,
  flattenProjectList,
  parseLayout,
  pickTerminal,
  projectDisplayName,
  projectMatches,
  switcherRows as splitSwitcherRows,
  type ActivityTier,
  type ProjectSignals,
} from '../models';
import { useSettingsStore } from '../state/settingsStore';
import { ActivityDot, TierHeader, useActivityRows } from '../components/projectActivity';
import { getOkenaNative, type OkenaNative, type ProjectInfo } from '../native/okena';
import { OkenaColors, TerminalTheme } from '../theme';
import { TerminalTabs } from '../components/TerminalTabs';
import { TerminalPane, type TerminalPaneHandle } from '../components/TerminalPane';
import { KeyToolbar, KeyModifiers } from '../components/KeyToolbar';
import { ProjectDrawer } from '../components/ProjectDrawer';
import { Icon } from '../components/Icon';
import { UsageIndicator } from '../components/UsageIndicator';
import { useNow } from '../components/useNow';
import { indicatorItems } from '../models/usage';
import { ConnectedUsageSheet } from './UsageSheet';
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
  const layoutJson = useWorkspaceStore((s) => s.projectLayoutJson);
  const secondsSinceActivity = useWorkspaceStore((s) => s.secondsSinceActivity);
  const selectProject = useWorkspaceStore((s) => s.selectProject);
  const selectTerminal = useWorkspaceStore((s) => s.selectTerminal);
  const restoreProjectMissing = useWorkspaceStore((s) => s.restoreProjectMissing);
  const clearRestoreProjectMissing = useWorkspaceStore((s) => s.clearRestoreProjectMissing);
  const previousProjectId = useWorkspaceStore((s) => s.previousProjectId);
  const signals = useWorkspaceStore((s) => s.projectSignals);
  const usage = useWorkspaceStore((s) => s.usage);
  const sortMode = useSettingsStore((s) => s.projectSortMode);
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
  const [usageOpen, setUsageOpen] = useState(false);

  // Pace moves with the clock, so re-derive the ring colors each minute too.
  const usageNow = useNow(60_000, usage !== null);
  const usageItems = useMemo(() => indicatorItems(usage, usageNow), [usage, usageNow]);

  // The remembered project is gone: show the project list instead.
  useEffect(() => {
    if (!restoreProjectMissing) return;
    setDrawerOpen(true);
    clearRestoreProjectMissing();
  }, [restoreProjectMissing, clearRestoreProjectMissing]);

  // Switcher rows in desktop sidebar order, filtered by the search field.
  const projectList = useMemo(
    () => buildProjectList(projects, folders, projectOrder),
    [projects, folders, projectOrder],
  );
  const manualRows = useMemo(
    () => flattenProjectList(filterProjectList(projectList, switcherQuery)),
    [projectList, switcherQuery],
  );
  const activityRows = useActivityRows(switcherOpen);
  // Activity order as switcher rows: `tier` set on a row that starts a tier
  // section (none for the current / previous rows on top, or while searching).
  const activitySwitcherRows = useMemo(() => {
    const needle = switcherQuery.trim().toLowerCase();
    if (needle.length > 0) {
      return activityRows
        .filter((r) => projectMatches(r.project, needle))
        .map((r) => ({ project: r.project, tier: undefined as ActivityTier | undefined }));
    }
    const { top, rest } = splitSwitcherRows(
      activityRows,
      project?.id ?? null,
      previousProjectId,
    );
    return [
      ...top.map((r) => ({ project: r.project, tier: undefined as ActivityTier | undefined })),
      ...rest.map((r, i) => ({
        project: r.project,
        tier: i === 0 || rest[i - 1]!.tier !== r.tier ? r.tier : undefined,
      })),
    ];
  }, [activityRows, switcherQuery, project?.id, previousProjectId]);
  const isActivity = sortMode === 'activity';
  const switcherCount = isActivity ? activitySwitcherRows.length : manualRows.length;
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

  // The one terminal on screen. The store keeps the selection valid; this only
  // covers the frame before its next poll.
  const activeTerminalId = project
    ? pickTerminal(project.terminalIds, [selectedTerminalId])
    : null;

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
  // Split next to the terminal on screen; the store auto-selects the new one.
  const onSplit = (direction: 'vertical' | 'horizontal') => {
    if (!connId || !project) return;
    const path =
      layoutNode && activeTerminalId ? (findTerminalPath(layoutNode, activeTerminalId) ?? []) : [];
    void native?.splitTerminal(connId, project.id, path, direction);
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
    if (!activeTerminalId) {
      return (
        <Centered>
          <Text style={styles.dim}>No terminals</Text>
          <Pressable style={styles.primaryBtn} onPress={onNewTerminal}>
            <Text style={styles.primaryBtnText}>New Terminal</Text>
          </Pressable>
        </Centered>
      );
    }
    // Keyed on the terminal: a fresh pane measures itself and sends the PTY
    // resize for the newly shown terminal (an in-place id swap would not).
    return (
      <TerminalPane
        key={activeTerminalId}
        ref={paneRef}
        connId={connId}
        terminalId={activeTerminalId}
        fonts={fonts}
        modifiers={modifiers}
        onSelect={selectTerminal}
        selected
        focusBlocked={drawerOpen || switcherOpen || actionsOpen || usageOpen}
        native={native}
      />
    );
  };

  const showToolbar = connId !== null && project !== null && activeTerminalId !== null;

  return (
    <View style={styles.root}>
      {/* App bar */}
      <View style={styles.appBar}>
        <Pressable hitSlop={8} style={styles.appBarBtn} onPress={() => setDrawerOpen(true)}>
          <Icon name="menu" size={22} color={OkenaColors.textPrimary} />
        </Pressable>
        <Pressable
          style={styles.titleWrap}
          disabled={projects.length <= 1}
          onPress={() => setSwitcherOpen(true)}
        >
          <Text style={styles.title} numberOfLines={1}>
            {projectName}
          </Text>
          {projects.length > 1 ? (
            <Icon name="caretDown" size={14} color={OkenaColors.textSecondary} style={styles.titleCaret} />
          ) : null}
        </Pressable>
        <View style={styles.flexSpacer} />
        {connId ? <UsageIndicator items={usageItems} onPress={() => setUsageOpen(true)} /> : null}
        {connId ? <View style={[styles.dot, { backgroundColor: dotColor }]} /> : null}
        {connId && project ? (
          <Pressable hitSlop={8} style={styles.appBarBtn} onPress={() => setActionsOpen(true)}>
            <Icon name="plus" size={22} color={OkenaColors.textPrimary} />
          </Pressable>
        ) : null}
      </View>

      {connId && project && project.terminalIds.length > 1 ? (
        <TerminalTabs
          project={project}
          activeTerminalId={activeTerminalId}
          onSelect={selectTerminal}
        />
      ) : null}

      {/* Body + key toolbar (toolbar rides above the keyboard) */}
      <KeyboardAvoidingView
        style={styles.flex}
        behavior={Platform.OS === 'ios' ? 'padding' : undefined}
        // The keyboard frame is in screen coordinates, but this view's frame is
        // relative to the safe-area container; offset by the top inset.
        keyboardVerticalOffset={insets.top}
      >
        <View style={styles.flex}>{renderBody()}</View>
        {showToolbar && connId && activeTerminalId ? (
          <KeyToolbar
            connId={connId}
            terminalId={activeTerminalId}
            modifiers={modifiers}
            native={native}
            onHideKeyboard={() => paneRef.current?.blur()}
            onShowKeyboard={() => paneRef.current?.focus()}
            onInputSent={() => {
              paneRef.current?.resetInput();
              paneRef.current?.scrollToBottom();
            }}
          />
        ) : null}
      </KeyboardAvoidingView>

      {/* Drawer */}
      <ProjectDrawer open={drawerOpen} onClose={() => setDrawerOpen(false)} native={native} />

      <ConnectedUsageSheet visible={usageOpen} onClose={() => setUsageOpen(false)} />

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
              {switcherCount === 0 ? (
                <Text style={[styles.menuItem, styles.dim]}>No matching projects</Text>
              ) : null}
              {isActivity
                ? activitySwitcherRows.map(({ project: p, tier }) => (
                    <React.Fragment key={p.id}>
                      {tier ? <TierHeader tier={tier} style={styles.menuTier} /> : null}
                      <SwitcherRow
                        project={p}
                        depth={0}
                        selected={p.id === project?.id}
                        previous={p.id === previousProjectId && p.id !== project?.id}
                        signals={signals[p.id]}
                        onPress={() => {
                          selectProject(p.id);
                          closeSwitcher();
                        }}
                      />
                    </React.Fragment>
                  ))
                : manualRows.map((row) => {
                    if (row.kind === 'folder') {
                      return (
                        <Text
                          key={`folder-${row.folder.id}`}
                          style={styles.menuFolder}
                          numberOfLines={1}
                        >
                          {row.folder.name}
                        </Text>
                      );
                    }
                    const { project: p, depth } = row;
                    return (
                      <SwitcherRow
                        key={p.id}
                        project={p}
                        depth={depth}
                        selected={p.id === project?.id}
                        previous={false}
                        signals={signals[p.id]}
                        onPress={() => {
                          selectProject(p.id);
                          closeSwitcher();
                        }}
                      />
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
          {activeTerminalId ? (
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
            </>
          ) : null}
        </View>
      </Modal>
    </View>
  );
};

const SwitcherRow: React.FC<{
  project: ProjectInfo;
  depth: number;
  selected: boolean;
  /** The previously viewed project (one tap back). */
  previous: boolean;
  signals?: ProjectSignals;
  onPress: () => void;
}> = ({ project: p, depth, selected, previous, signals, onPress }) => (
  <Pressable style={[styles.menuItem, styles.menuRow, { paddingLeft: 16 + depth * 12 }]} onPress={onPress}>
    <View style={styles.flexShrink}>
      <Text style={[styles.menuItemText, selected && styles.menuItemSelected]} numberOfLines={1}>
        {projectDisplayName(p)}
      </Text>
      {p.gitBranch && !p.parentProjectId ? (
        <View style={styles.menuBranchRow}>
          <Icon name="gitBranch" size={11} color={OkenaColors.textTertiary} />
          <Text style={styles.menuBranch} numberOfLines={1}>
            {p.gitBranch}
          </Text>
        </View>
      ) : null}
    </View>
    <View style={styles.flexSpacer} />
    {selected || previous ? (
      <Text style={styles.menuTag}>{selected ? 'current' : 'previous'}</Text>
    ) : null}
    <ActivityDot signals={signals} />
  </Pressable>
);

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
  titleWrap: { flexDirection: 'row', alignItems: 'center', flexShrink: 1 },
  title: { color: OkenaColors.textPrimary, fontSize: 17, fontWeight: '600', flexShrink: 1 },
  titleCaret: { marginLeft: 4 },
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
  menuRow: { flexDirection: 'row', alignItems: 'center' },
  menuTier: { paddingTop: 10, paddingBottom: 2 },
  menuTag: { color: OkenaColors.textTertiary, fontSize: 11, marginLeft: 8 },
  flexShrink: { flexShrink: 1 },
  menuItemSelected: { color: OkenaColors.accent, fontWeight: '700' },
  menuBranchRow: { flexDirection: 'row', alignItems: 'center', marginTop: 2 },
  menuBranch: { color: OkenaColors.textTertiary, fontSize: 11, marginLeft: 4, flexShrink: 1 },
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
