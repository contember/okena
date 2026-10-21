/**
 * ProjectDrawer.tsx — the slide-in project drawer.
 *
 * Port of `mobile/lib/src/widgets/project_drawer.dart`. A custom slide-in drawer
 * (an absolutely-positioned overlay animated with `Animated`, NOT
 * `@react-navigation/drawer`) rendered above the workspace. It shows:
 *   - a header (app name + active server name + a small status dot),
 *   - a search field filtering projects by name / path (case-insensitive),
 *   - the project list, in the order the "Project order" setting picks:
 *     - Activity (default): every project flat under PINNED / NEEDS ATTENTION /
 *       RUNNING / RECENT headers, like the desktop sidebar's activity view
 *       (`models/activityOrder.ts`). The order freezes while the drawer is
 *       open; status dots keep updating.
 *     - Manual: standalone projects + folders (folder header + its indented
 *       projects), worktrees nested under their parent, matching the desktop
 *       sidebar's manual view (`models/projectList.ts`).
 *     Each row carries an activity dot (amber: needs attention, green: running).
 *   - tapping a project selects it (`selectProject`) and expands it inline to
 *     show its terminals; tapping a terminal selects + focuses it and closes the
 *     drawer. A terminal with a connected `claude` or open Claude dialogs has a
 *     "Chat" pill that opens it as its Claude chat.
 *   - long-press a project opens an actions sheet: change color (color picker),
 *     and move up/down within its folder (reorder).
 *   - "Add Project" (name + path dialog) and "Disconnect" at the bottom.
 *
 * Presentational + injected `native` (defaults to `getOkenaNative()`).
 * Orchestration is via the stores (`useWorkspaceStore` / `useConnectionStore`).
 * StatusIndicator is intentionally NOT imported (owned by another agent) — a
 * tiny inline dot is used instead.
 */

import React, { useEffect, useMemo, useRef, useState } from 'react';
import {
  View,
  Text,
  Pressable,
  FlatList,
  TextInput,
  Modal,
  Animated,
  StyleSheet,
  useWindowDimensions,
} from 'react-native';

import { useWorkspaceStore, useConnectionStore } from '../state';
import type { FolderInfo, OkenaNative, ProjectInfo } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import {
  buildProjectList,
  filterProjectList,
  flattenProjectList,
  projectDisplayName,
  projectMatches,
  type ProjectListRow,
  type ProjectSignals,
} from '../models';
import { useSettingsStore } from '../state/settingsStore';
import { ActivityDot, TierHeader, useActivityRows } from './projectActivity';
import { OkenaColors } from '../theme';
import { Icon } from './Icon';
import { ClaudeIndicator } from './ClaudeIndicator';
import { SettingsSheet } from '../screens/SettingsSheet';
import { ConnectedUsageSheet } from '../screens/UsageSheet';
import { SettingsIcon } from './KeyIcons';

// ── Folder colors (mirror _folderColorToColor in project_drawer.dart) ────────

const COLOR_OPTIONS = [
  'red',
  'orange',
  'yellow',
  'lime',
  'green',
  'teal',
  'cyan',
  'blue',
  'purple',
  'pink',
] as const;

const COLOR_HEX: Record<string, string> = {
  red: '#f44336',
  orange: '#ff9800',
  yellow: '#ffeb3b',
  lime: '#cddc39',
  green: '#4caf50',
  teal: '#009688',
  cyan: '#00bcd4',
  blue: '#2196f3',
  purple: '#9c27b0',
  pink: '#e91e63',
};

export function folderColor(name: string): string {
  return COLOR_HEX[name] ?? OkenaColors.textTertiary;
}

const DRAWER_WIDTH_FRACTION = 0.82;
const DRAWER_MAX_WIDTH = 360;
const INDENT_STEP = 16;

type DrawerNative = Pick<
  OkenaNative,
  | 'focusTerminal'
  | 'setProjectColor'
  | 'setFolderColor'
  | 'addProject'
  | 'reorderProjectInFolder'
  | 'closeTerminal'
  | 'createTerminal'
>;

export interface ProjectDrawerProps {
  open: boolean;
  onClose: () => void;
  native?: DrawerNative;
}

export const ProjectDrawer: React.FC<ProjectDrawerProps> = ({
  open,
  onClose,
  native = getOkenaNative(),
}) => {
  const { width } = useWindowDimensions();
  const drawerWidth = Math.min(width * DRAWER_WIDTH_FRACTION, DRAWER_MAX_WIDTH);

  const slide = useRef(new Animated.Value(0)).current;
  // Keep the Modal mounted through the close animation.
  const [mounted, setMounted] = useState(open);

  const [query, setQuery] = useState('');

  useEffect(() => {
    if (open) setMounted(true);
    else setQuery('');
    Animated.timing(slide, {
      toValue: open ? 1 : 0,
      duration: 220,
      useNativeDriver: true,
    }).start(({ finished }) => {
      if (finished && !open) setMounted(false);
    });
  }, [open, slide]);

  const projects = useWorkspaceStore((s) => s.projects);
  const folders = useWorkspaceStore((s) => s.folders);
  const projectOrder = useWorkspaceStore((s) => s.projectOrder);
  const selectedProjectId = useWorkspaceStore((s) => s.selectedProjectId);
  const selectedTerminalId = useWorkspaceStore((s) => s.selectedTerminalId);
  const selectProject = useWorkspaceStore((s) => s.selectProject);
  const selectTerminal = useWorkspaceStore((s) => s.selectTerminal);
  const signals = useWorkspaceStore((s) => s.projectSignals);
  const claudeDialogs = useWorkspaceStore((s) => s.claudeDialogs);
  const claudeSessions = useWorkspaceStore((s) => s.claudeSessions);
  const setTerminalView = useWorkspaceStore((s) => s.setTerminalView);
  const sortMode = useSettingsStore((s) => s.projectSortMode);

  const connId = useConnectionStore((s) => s.connId);
  const activeServer = useConnectionStore((s) => s.activeServer);
  const status = useConnectionStore((s) => s.status);
  const disconnect = useConnectionStore((s) => s.disconnect);

  const [addOpen, setAddOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [usageOpen, setUsageOpen] = useState(false);
  const [colorPicker, setColorPicker] = useState<{
    current: string;
    onSelect: (color: string) => void;
  } | null>(null);
  const [reorder, setReorder] = useState<{
    project: ProjectInfo;
    folderId: string;
    index: number;
    total: number;
  } | null>(null);

  // Build the ordered display list (folders + standalone projects), then filter.
  const allItems = useMemo(
    () => buildProjectList(projects, folders, projectOrder),
    [projects, folders, projectOrder],
  );
  const items = useMemo(
    () => filterProjectList(allItems, query),
    [allItems, query],
  );
  const manualRows = useMemo(() => flattenProjectList(items), [items]);
  const projectFolders = useMemo(() => {
    const byId = new Map<string, FolderInfo>();
    for (const item of allItems) {
      if (item.kind === 'folder') {
        for (const node of item.projects)
          byId.set(node.project.id, item.folder);
      }
    }
    return byId;
  }, [allItems]);
  // Activity order, frozen while the drawer is on screen (incl. its close).
  const allActivityRows = useActivityRows(open || mounted);
  const activityRows = useMemo(() => {
    const needle = query.trim().toLowerCase();
    if (needle.length === 0) return allActivityRows;
    return allActivityRows.filter((r) => projectMatches(r.project, needle));
  }, [allActivityRows, query]);
  const isActivity = sortMode === 'activity';
  const rows: ProjectListRow[] = useMemo(
    () =>
      isActivity
        ? activityRows.map(({ project }) => ({
            kind: 'project',
            project,
            depth: 0,
          }))
        : manualRows,
    [isActivity, activityRows, manualRows],
  );

  if (!mounted) return null;

  const translateX = slide.interpolate({
    inputRange: [0, 1],
    outputRange: [-drawerWidth, 0],
  });
  const backdropOpacity = slide.interpolate({
    inputRange: [0, 1],
    outputRange: [0, 1],
  });

  const handleSelectProject = (project: ProjectInfo) => {
    selectProject(project.id);
  };

  const handleSelectTerminal = (project: ProjectInfo, terminalId: string) => {
    selectTerminal(terminalId);
    if (connId) void native.focusTerminal(connId, project.id, terminalId);
    onClose();
  };

  const handleOpenChat = (project: ProjectInfo, terminalId: string) => {
    setTerminalView(terminalId, 'chat');
    handleSelectTerminal(project, terminalId);
  };

  const openColorPickerForProject = (project: ProjectInfo) => {
    if (!connId) return;
    setColorPicker({
      current: project.folderColor,
      onSelect: (color) =>
        void native.setProjectColor(connId, project.id, color),
    });
  };

  const openColorPickerForFolder = (folder: FolderInfo) => {
    if (!connId) return;
    setColorPicker({
      current: folder.folderColor,
      onSelect: (color) => void native.setFolderColor(connId, folder.id, color),
    });
  };

  return (
    <Modal visible transparent animationType="none" onRequestClose={onClose}>
      <Animated.View style={[styles.backdrop, { opacity: backdropOpacity }]}>
        <Pressable style={StyleSheet.absoluteFill} onPress={onClose} />
      </Animated.View>

      <Animated.View
        style={[
          styles.drawer,
          { width: drawerWidth, transform: [{ translateX }] },
        ]}
      >
        {/* Header */}
        <View style={styles.header}>
          <Text style={styles.headerTitle}>Okena</Text>
          {activeServer ? (
            <Text style={styles.headerSubtitle}>
              {`${activeServer.host}:${activeServer.port}`}
            </Text>
          ) : null}
          <View style={styles.flexSpacer} />
          <View style={styles.statusRow}>
            <StatusDot status={status.kind} />
            <Text style={styles.statusText}>{status.kind}</Text>
          </View>
        </View>

        {/* Search */}
        <View style={styles.searchWrap}>
          <TextInput
            style={styles.searchInput}
            placeholder="Search projects"
            placeholderTextColor={OkenaColors.textTertiary}
            value={query}
            onChangeText={setQuery}
            autoCapitalize="none"
            autoCorrect={false}
            spellCheck={false}
            clearButtonMode="while-editing"
            returnKeyType="search"
            keyboardAppearance="dark"
          />
        </View>

        {/* Project / folder list */}
        <FlatList
          style={styles.flex}
          data={rows}
          initialNumToRender={12}
          maxToRenderPerBatch={12}
          windowSize={5}
          keyboardShouldPersistTaps="handled"
          keyboardDismissMode="on-drag"
          automaticallyAdjustKeyboardInsets
          keyExtractor={(row) =>
            row.kind === 'folder' ? `folder-${row.folder.id}` : row.project.id
          }
          ListEmptyComponent={
            query.trim() ? (
              <Text style={styles.emptyText}>No matching projects</Text>
            ) : null
          }
          renderItem={({ item, index }) => {
            if (item.kind === 'folder') {
              const color = folderColor(item.folder.folderColor);
              return (
                <Pressable
                  style={styles.folderHeader}
                  onLongPress={() => openColorPickerForFolder(item.folder)}
                >
                  <Icon
                    name="caretRight"
                    size={12}
                    color={color}
                    style={styles.folderIcon}
                  />
                  <Text style={[styles.folderName, { color }]}>
                    {item.folder.name}
                  </Text>
                </Pressable>
              );
            }
            const activity = isActivity ? activityRows[index] : undefined;
            return (
              <>
                {activity &&
                (index === 0 ||
                  activityRows[index - 1]?.tier !== activity.tier) ? (
                  <TierHeader tier={activity.tier} />
                ) : null}
                <ProjectRow
                  project={item.project}
                  depth={item.depth}
                  signals={signals}
                  selectedProjectId={selectedProjectId}
                  selectedTerminalId={selectedTerminalId}
                  connId={connId}
                  native={native}
                  onSelectProject={handleSelectProject}
                  onSelectTerminal={handleSelectTerminal}
                  onOpenChat={handleOpenChat}
                  claudeDialogs={claudeDialogs}
                  claudeSessions={claudeSessions}
                  onLongPress={() => {
                    const folder = !isActivity
                      ? projectFolders.get(item.project.id)
                      : undefined;
                    if (folder) {
                      const folderIndex = folder.projectIds.indexOf(
                        item.project.id,
                      );
                      if (folderIndex >= 0)
                        setReorder({
                          project: item.project,
                          folderId: folder.id,
                          index: folderIndex,
                          total: folder.projectIds.length,
                        });
                    } else openColorPickerForProject(item.project);
                  }}
                />
              </>
            );
          }}
        />

        {/* Footer */}
        <View style={styles.divider} />
        {connId ? (
          <Pressable style={styles.footerItem} onPress={() => setAddOpen(true)}>
            <View style={styles.footerIcon}>
              <Icon name="plus" size={18} color={OkenaColors.textSecondary} />
            </View>
            <Text style={styles.footerText}>Add Project</Text>
          </Pressable>
        ) : null}
        <Pressable
          style={styles.footerItem}
          onPress={() => setSettingsOpen(true)}
          accessibilityRole="button"
        >
          <View style={styles.footerIconBox}>
            <SettingsIcon
              color={OkenaColors.textSecondary}
              background={OkenaColors.surface}
              size={16}
            />
          </View>
          <Text style={styles.footerText}>Settings</Text>
        </Pressable>
        {connId ? (
          <Pressable
            style={styles.footerItem}
            onPress={() => setUsageOpen(true)}
            accessibilityRole="button"
          >
            <View style={styles.footerIcon}>
              <Icon name="gauge" size={18} color={OkenaColors.textSecondary} />
            </View>
            <Text style={styles.footerText}>Usage</Text>
          </Pressable>
        ) : null}
        <Pressable
          style={styles.footerItem}
          onPress={() => {
            onClose();
            disconnect();
          }}
        >
          <View style={styles.footerIcon}>
            <Icon
              name="disconnect"
              size={18}
              color={OkenaColors.textSecondary}
            />
          </View>
          <Text style={styles.footerText}>Disconnect</Text>
        </Pressable>
      </Animated.View>

      <SettingsSheet
        visible={settingsOpen}
        onClose={() => setSettingsOpen(false)}
      />
      <ConnectedUsageSheet
        visible={usageOpen}
        onClose={() => setUsageOpen(false)}
      />

      {/* Add Project dialog */}
      <AddProjectDialog
        visible={addOpen}
        onClose={() => setAddOpen(false)}
        onSubmit={(name, path) => {
          if (connId) void native.addProject(connId, name, path);
        }}
      />

      {/* Color picker */}
      <ColorPicker
        visible={colorPicker !== null}
        current={colorPicker?.current ?? ''}
        onClose={() => setColorPicker(null)}
        onSelect={(color) => {
          colorPicker?.onSelect(color);
          setColorPicker(null);
        }}
      />

      {/* Reorder / color action sheet for a folder project */}
      <ReorderSheet
        info={reorder}
        onClose={() => setReorder(null)}
        onChangeColor={() => {
          const r = reorder;
          setReorder(null);
          if (r) openColorPickerForProject(r.project);
        }}
        onMove={(newIndex) => {
          const r = reorder;
          setReorder(null);
          if (r && connId) {
            void native.reorderProjectInFolder(
              connId,
              r.folderId,
              r.project.id,
              newIndex,
            );
          }
        }}
      />
    </Modal>
  );
};

// ── Folder section ────────────────────────────────────────────────────────

interface RowCommon {
  /** Activity signals per project id (status dot). */
  signals: Record<string, ProjectSignals>;
  selectedProjectId: string | null;
  selectedTerminalId: string | null;
  connId: string | null;
  native: DrawerNative;
  onSelectProject: (project: ProjectInfo) => void;
  onSelectTerminal: (project: ProjectInfo, terminalId: string) => void;
  onOpenChat: (project: ProjectInfo, terminalId: string) => void;
  /** Open Claude dialogs per terminal id. */
  claudeDialogs: Record<string, number>;
  /** Terminals with a connected `claude`. */
  claudeSessions: string[];
}

// ── Project row (+ inline expansion when selected) ──────────────────────────

const ProjectRow: React.FC<
  RowCommon & {
    project: ProjectInfo;
    /** Nesting level: folder member and/or worktree. */
    depth: number;
    onLongPress: () => void;
  }
> = ({
  project,
  depth,
  signals,
  selectedProjectId,
  selectedTerminalId,
  connId,
  native,
  onSelectProject,
  onSelectTerminal,
  onOpenChat,
  claudeDialogs,
  claudeSessions,
  onLongPress,
}) => {
  const isSelected = project.id === selectedProjectId;
  const color = folderColor(project.folderColor);
  const runningServices = project.services.filter(
    (s) => s.status === 'running',
  ).length;

  return (
    <View>
      <Pressable
        style={[styles.projectRow, isSelected && styles.projectRowSelected]}
        onPress={() => onSelectProject(project)}
        onLongPress={onLongPress}
      >
        <Icon
          name={project.parentProjectId ? 'gitBranch' : 'caretRight'}
          size={project.parentProjectId ? 15 : 14}
          color={isSelected ? OkenaColors.accent : color}
          style={[
            styles.projectIcon,
            depth > 0 && { marginLeft: INDENT_STEP * depth },
          ]}
        />
        <View style={styles.flex}>
          <Text style={styles.projectName} numberOfLines={1}>
            {projectDisplayName(project)}
          </Text>
          {(project.gitBranch && !project.parentProjectId) ||
          runningServices > 0 ? (
            <View style={styles.subtitleRow}>
              {project.gitBranch && !project.parentProjectId ? (
                <>
                  <Icon
                    name="gitBranch"
                    size={11}
                    color={OkenaColors.textTertiary}
                  />
                  <Text
                    style={[styles.subtitleText, styles.subtitleBranch]}
                    numberOfLines={1}
                  >
                    {project.gitBranch}
                  </Text>
                </>
              ) : null}
              {runningServices > 0 ? (
                <>
                  <View
                    style={[
                      styles.runningDot,
                      project.gitBranch &&
                        !project.parentProjectId &&
                        styles.runningDotSpaced,
                    ]}
                  />
                  <Text style={[styles.subtitleText, styles.subtitleRunning]}>
                    {runningServices}
                  </Text>
                </>
              ) : null}
            </View>
          ) : null}
        </View>
        <ActivityDot signals={signals[project.id]} />
      </Pressable>

      {isSelected ? (
        <View>
          {project.terminalIds.map((tid, idx) => {
            const isTerminalSelected = tid === selectedTerminalId;
            const name = project.terminalNames[tid] ?? `Terminal ${idx + 1}`;
            const agent = project.terminalAgentStatus[tid];
            const dialogs = claudeDialogs[tid] ?? 0;
            return (
              <Pressable
                key={tid}
                style={styles.terminalRow}
                onPress={() => onSelectTerminal(project, tid)}
              >
                <Icon
                  name="prompt"
                  size={14}
                  color={
                    isTerminalSelected
                      ? OkenaColors.accent
                      : OkenaColors.textSecondary
                  }
                  style={styles.terminalIcon}
                />
                <Text
                  style={[
                    styles.terminalName,
                    isTerminalSelected && styles.terminalSelected,
                  ]}
                  numberOfLines={1}
                >
                  {name}
                </Text>
                <View style={styles.flexSpacer} />
                {claudeSessions.includes(tid) || dialogs > 0 ? (
                  <ClaudeIndicator
                    lifecycle={agent?.lifecycle}
                    dialogs={dialogs}
                    onPress={() => onOpenChat(project, tid)}
                  />
                ) : null}
                <Pressable
                  hitSlop={8}
                  onPress={() => {
                    if (connId)
                      void native.closeTerminal(connId, project.id, tid);
                  }}
                >
                  <Icon
                    name="close"
                    size={14}
                    color={OkenaColors.textTertiary}
                  />
                </Pressable>
              </Pressable>
            );
          })}
          {connId ? (
            <Pressable
              style={styles.terminalRow}
              onPress={() => {
                void native.createTerminal(connId, project.id);
              }}
            >
              <Icon
                name="plus"
                size={14}
                color={OkenaColors.textSecondary}
                style={styles.terminalIcon}
              />
              <Text style={styles.terminalName}>New Terminal</Text>
            </Pressable>
          ) : null}
        </View>
      ) : null}
    </View>
  );
};

// ── Small inline status dot (StatusIndicator is owned by another agent) ──────

const StatusDot: React.FC<{ status: string }> = ({ status }) => {
  const color =
    status === 'connected'
      ? OkenaColors.success
      : status === 'error'
      ? OkenaColors.error
      : OkenaColors.warning;
  return <View style={[styles.dot, { backgroundColor: color }]} />;
};

// ── Add Project dialog ──────────────────────────────────────────────────────

const AddProjectDialog: React.FC<{
  visible: boolean;
  onClose: () => void;
  onSubmit: (name: string, path: string) => void;
}> = ({ visible, onClose, onSubmit }) => {
  const [name, setName] = useState('');
  const [path, setPath] = useState('');

  useEffect(() => {
    if (visible) {
      setName('');
      setPath('');
    }
  }, [visible]);

  const submit = () => {
    const n = name.trim();
    const p = path.trim();
    if (n.length > 0 && p.length > 0) {
      onSubmit(n, p);
      onClose();
    }
  };

  return (
    <Modal
      visible={visible}
      transparent
      animationType="fade"
      onRequestClose={onClose}
    >
      <View style={styles.dialogBackdrop}>
        <View style={styles.dialog}>
          <Text style={styles.dialogTitle}>Add Project</Text>
          <TextInput
            style={styles.dialogInput}
            placeholder="Name"
            placeholderTextColor={OkenaColors.textTertiary}
            value={name}
            onChangeText={setName}
            autoFocus
          />
          <TextInput
            style={styles.dialogInput}
            placeholder="/home/user/project"
            placeholderTextColor={OkenaColors.textTertiary}
            value={path}
            onChangeText={setPath}
            autoCapitalize="none"
            autoCorrect={false}
          />
          <View style={styles.dialogActions}>
            <Pressable style={styles.dialogBtn} onPress={onClose}>
              <Text style={styles.dialogBtnText}>Cancel</Text>
            </Pressable>
            <Pressable style={styles.dialogBtn} onPress={submit}>
              <Text style={[styles.dialogBtnText, styles.dialogBtnPrimary]}>
                Add
              </Text>
            </Pressable>
          </View>
        </View>
      </View>
    </Modal>
  );
};

// ── Color picker ─────────────────────────────────────────────────────────────

const ColorPicker: React.FC<{
  visible: boolean;
  current: string;
  onClose: () => void;
  onSelect: (color: string) => void;
}> = ({ visible, current, onClose, onSelect }) => (
  <Modal
    visible={visible}
    transparent
    animationType="slide"
    onRequestClose={onClose}
  >
    <Pressable style={styles.sheetBackdrop} onPress={onClose} />
    <View style={styles.sheet}>
      <Text style={styles.sheetTitle}>Choose Color</Text>
      <View style={styles.swatchWrap}>
        {COLOR_OPTIONS.map((name) => {
          const selected = name === current;
          return (
            <Pressable
              key={name}
              style={[
                styles.swatch,
                { backgroundColor: folderColor(name) },
                selected && styles.swatchSelected,
              ]}
              onPress={() => onSelect(name)}
            >
              {selected ? (
                <Icon name="check" size={18} strokeWidth={3} color="#ffffff" />
              ) : null}
            </Pressable>
          );
        })}
      </View>
    </View>
  </Modal>
);

// ── Reorder action sheet ────────────────────────────────────────────────────

const ReorderSheet: React.FC<{
  info: {
    project: ProjectInfo;
    folderId: string;
    index: number;
    total: number;
  } | null;
  onClose: () => void;
  onChangeColor: () => void;
  /**
   * `newIndex` is an insert-before position in the original list, matching the
   * daemon's `reorder_project_in_folder` (it subtracts one when moving down).
   */
  onMove: (newIndex: number) => void;
}> = ({ info, onClose, onChangeColor, onMove }) => {
  if (!info) return null;
  const { project, index, total } = info;
  return (
    <Modal visible transparent animationType="slide" onRequestClose={onClose}>
      <Pressable style={styles.sheetBackdrop} onPress={onClose} />
      <View style={styles.sheet}>
        <Text style={styles.sheetTitle}>{project.name}</Text>
        <Pressable style={styles.sheetItem} onPress={onChangeColor}>
          <Text style={styles.sheetItemText}>Change Color</Text>
        </Pressable>
        {index > 0 ? (
          <Pressable style={styles.sheetItem} onPress={() => onMove(index - 1)}>
            <Text style={styles.sheetItemText}>Move Up</Text>
          </Pressable>
        ) : null}
        {index < total - 1 ? (
          <Pressable style={styles.sheetItem} onPress={() => onMove(index + 2)}>
            <Text style={styles.sheetItemText}>Move Down</Text>
          </Pressable>
        ) : null}
        {index > 0 ? (
          <Pressable style={styles.sheetItem} onPress={() => onMove(0)}>
            <Text style={styles.sheetItemText}>Move to Top</Text>
          </Pressable>
        ) : null}
        {index < total - 1 ? (
          <Pressable style={styles.sheetItem} onPress={() => onMove(total)}>
            <Text style={styles.sheetItemText}>Move to Bottom</Text>
          </Pressable>
        ) : null}
      </View>
    </Modal>
  );
};

const styles = StyleSheet.create({
  flex: { flex: 1 },
  flexSpacer: { flex: 1 },
  backdrop: { ...StyleSheet.absoluteFill, backgroundColor: 'rgba(0,0,0,0.5)' },
  drawer: {
    position: 'absolute',
    top: 0,
    bottom: 0,
    left: 0,
    backgroundColor: OkenaColors.surface,
  },
  header: {
    height: 140,
    paddingHorizontal: 16,
    paddingTop: 48,
    paddingBottom: 12,
    backgroundColor: OkenaColors.surfaceElevated,
    justifyContent: 'flex-start',
  },
  headerTitle: {
    color: OkenaColors.textPrimary,
    fontSize: 22,
    fontWeight: '700',
  },
  headerSubtitle: {
    color: OkenaColors.textSecondary,
    fontSize: 12,
    marginTop: 4,
  },
  statusRow: { flexDirection: 'row', alignItems: 'center' },
  statusText: { color: OkenaColors.textSecondary, fontSize: 12, marginLeft: 6 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  folderHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingLeft: 16,
    paddingRight: 16,
    paddingTop: 12,
    paddingBottom: 4,
  },
  folderIcon: { marginRight: 8 },
  folderName: { fontSize: 12, fontWeight: '600', letterSpacing: 0.5 },
  projectRow: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 12,
    paddingHorizontal: 16,
  },
  projectRowSelected: { backgroundColor: OkenaColors.surfaceOverlay },
  projectIcon: { marginRight: 12 },
  searchWrap: { paddingHorizontal: 12, paddingVertical: 8 },
  searchInput: {
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 8,
    color: OkenaColors.textPrimary,
    fontSize: 14,
  },
  emptyText: {
    color: OkenaColors.textTertiary,
    fontSize: 13,
    paddingHorizontal: 16,
    paddingVertical: 12,
  },
  projectName: { color: OkenaColors.textPrimary, fontSize: 15 },
  subtitleRow: { flexDirection: 'row', alignItems: 'center', marginTop: 2 },
  subtitleText: { color: OkenaColors.textTertiary, fontSize: 11 },
  subtitleBranch: { marginLeft: 3, flexShrink: 1 },
  subtitleRunning: { color: OkenaColors.success },
  runningDot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    marginRight: 4,
    backgroundColor: OkenaColors.success,
  },
  runningDotSpaced: { marginLeft: 10 },
  terminalRow: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 8,
    paddingLeft: 56,
    paddingRight: 12,
  },
  terminalIcon: { marginRight: 8 },
  terminalName: { color: OkenaColors.textPrimary, fontSize: 14 },
  terminalSelected: { color: OkenaColors.accent },
  divider: {
    height: StyleSheet.hairlineWidth,
    backgroundColor: OkenaColors.border,
  },
  footerItem: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 14,
    paddingHorizontal: 16,
  },
  footerIcon: { width: 28 },
  footerIconBox: { width: 28 },
  footerText: { color: OkenaColors.textPrimary, fontSize: 15 },
  // dialog
  dialogBackdrop: {
    flex: 1,
    backgroundColor: 'rgba(0,0,0,0.5)',
    alignItems: 'center',
    justifyContent: 'center',
    padding: 24,
  },
  dialog: {
    width: '100%',
    maxWidth: 360,
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 12,
    padding: 20,
  },
  dialogTitle: {
    color: OkenaColors.textPrimary,
    fontSize: 18,
    fontWeight: '600',
    marginBottom: 16,
  },
  dialogInput: {
    backgroundColor: OkenaColors.surface,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    color: OkenaColors.textPrimary,
    marginBottom: 12,
  },
  dialogActions: {
    flexDirection: 'row',
    justifyContent: 'flex-end',
    marginTop: 4,
  },
  dialogBtn: { paddingHorizontal: 16, paddingVertical: 8, marginLeft: 8 },
  dialogBtnText: { color: OkenaColors.textSecondary, fontSize: 14 },
  dialogBtnPrimary: { color: OkenaColors.accent, fontWeight: '600' },
  // sheets
  sheetBackdrop: { flex: 1, backgroundColor: 'rgba(0,0,0,0.4)' },
  sheet: {
    backgroundColor: OkenaColors.surfaceElevated,
    borderTopLeftRadius: 16,
    borderTopRightRadius: 16,
    padding: 16,
    paddingBottom: 32,
  },
  sheetTitle: {
    color: OkenaColors.textPrimary,
    fontSize: 16,
    fontWeight: '600',
    marginBottom: 12,
  },
  sheetItem: { paddingVertical: 14 },
  sheetItemText: { color: OkenaColors.textPrimary, fontSize: 15 },
  swatchWrap: { flexDirection: 'row', flexWrap: 'wrap', gap: 12 },
  swatch: {
    width: 40,
    height: 40,
    borderRadius: 20,
    alignItems: 'center',
    justifyContent: 'center',
  },
  swatchSelected: { borderWidth: 3, borderColor: '#ffffff' },
});

export default ProjectDrawer;
