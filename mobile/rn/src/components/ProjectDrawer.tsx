/**
 * ProjectDrawer.tsx — the slide-in project drawer.
 *
 * Port of `mobile/lib/src/widgets/project_drawer.dart`. A custom slide-in drawer
 * (an absolutely-positioned overlay animated with `Animated`, NOT
 * `@react-navigation/drawer`) rendered above the workspace. It shows:
 *   - a header (app name + active server name + a small status dot),
 *   - the ordered project list: standalone projects + folders (folder header +
 *     its indented projects). Ordering follows `projectOrder`, with any
 *     leftover projects appended (matches the Dart `_ProjectList`).
 *   - tapping a project selects it (`selectProject`) and expands it inline to
 *     show its terminals; tapping a terminal selects + focuses it and closes the
 *     drawer.
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
  ScrollView,
  TextInput,
  Modal,
  Animated,
  StyleSheet,
  useWindowDimensions,
} from 'react-native';

import {
  useWorkspaceStore,
  useConnectionStore,
} from '../state';
import type { FolderInfo, OkenaNative, ProjectInfo } from '../native/okena';
import { getOkenaNative } from '../native/okena';
import { OkenaColors } from '../theme';

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

function folderColor(name: string): string {
  return COLOR_HEX[name] ?? OkenaColors.textTertiary;
}

const DRAWER_WIDTH_FRACTION = 0.82;
const DRAWER_MAX_WIDTH = 360;

export interface ProjectDrawerProps {
  open: boolean;
  onClose: () => void;
  native?: OkenaNative;
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

  useEffect(() => {
    if (open) setMounted(true);
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

  const connId = useConnectionStore((s) => s.connId);
  const activeServer = useConnectionStore((s) => s.activeServer);
  const status = useConnectionStore((s) => s.status);
  const disconnect = useConnectionStore((s) => s.disconnect);

  const [addOpen, setAddOpen] = useState(false);
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

  // Build the ordered display list (folders + standalone projects).
  const items = useMemo(
    () => buildOrderedItems(projects, folders, projectOrder),
    [projects, folders, projectOrder],
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

  const openColorPickerForProject = (project: ProjectInfo) => {
    if (!connId) return;
    setColorPicker({
      current: project.folderColor,
      onSelect: (color) => void native.setProjectColor(connId, project.id, color),
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
        style={[styles.drawer, { width: drawerWidth, transform: [{ translateX }] }]}
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

        {/* Project / folder list */}
        <ScrollView style={styles.flex}>
          {items.map((item) =>
            item.kind === 'folder' ? (
              <FolderSection
                key={`folder-${item.folder.id}`}
                folder={item.folder}
                projects={item.projects}
                selectedProjectId={selectedProjectId}
                selectedTerminalId={selectedTerminalId}
                connId={connId}
                native={native}
                onSelectProject={handleSelectProject}
                onSelectTerminal={handleSelectTerminal}
                onLongPressFolder={() => openColorPickerForFolder(item.folder)}
                onLongPressProject={(project, index) =>
                  setReorder({
                    project,
                    folderId: item.folder.id,
                    index,
                    total: item.projects.length,
                  })
                }
              />
            ) : (
              <ProjectRow
                key={`project-${item.project.id}`}
                project={item.project}
                indent={false}
                selectedProjectId={selectedProjectId}
                selectedTerminalId={selectedTerminalId}
                connId={connId}
                native={native}
                onSelectProject={handleSelectProject}
                onSelectTerminal={handleSelectTerminal}
                onLongPress={() => openColorPickerForProject(item.project)}
              />
            ),
          )}
        </ScrollView>

        {/* Footer */}
        <View style={styles.divider} />
        {connId ? (
          <Pressable style={styles.footerItem} onPress={() => setAddOpen(true)}>
            <Text style={styles.footerIcon}>{'＋'}</Text>
            <Text style={styles.footerText}>Add Project</Text>
          </Pressable>
        ) : null}
        <Pressable
          style={styles.footerItem}
          onPress={() => {
            onClose();
            disconnect();
          }}
        >
          <Text style={styles.footerIcon}>{'⃠'}</Text>
          <Text style={styles.footerText}>Disconnect</Text>
        </Pressable>
      </Animated.View>

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
            void native.reorderProjectInFolder(connId, r.folderId, r.project.id, newIndex);
          }
        }}
      />
    </Modal>
  );
};

// ── Ordered list builder (mirrors _ProjectList in project_drawer.dart) ───────

type DisplayItem =
  | { kind: 'folder'; folder: FolderInfo; projects: ProjectInfo[] }
  | { kind: 'project'; project: ProjectInfo };

function buildOrderedItems(
  projects: ProjectInfo[],
  folders: FolderInfo[],
  projectOrder: string[],
): DisplayItem[] {
  const items: DisplayItem[] = [];

  if (projectOrder.length > 0 || folders.length > 0) {
    const folderMap = new Map(folders.map((f) => [f.id, f]));
    const projectMap = new Map(projects.map((p) => [p.id, p]));
    const displayed = new Set<string>();

    for (const entryId of projectOrder) {
      const folder = folderMap.get(entryId);
      if (folder) {
        const folderProjects = folder.projectIds
          .map((pid) => projectMap.get(pid))
          .filter((p): p is ProjectInfo => p !== undefined);
        items.push({ kind: 'folder', folder, projects: folderProjects });
        folder.projectIds.forEach((pid) => displayed.add(pid));
      } else {
        const project = projectMap.get(entryId);
        if (project) {
          items.push({ kind: 'project', project });
          displayed.add(entryId);
        }
      }
    }
    // Append any projects not in the order.
    for (const p of projects) {
      if (!displayed.has(p.id)) items.push({ kind: 'project', project: p });
    }
  } else {
    for (const p of projects) items.push({ kind: 'project', project: p });
  }

  return items;
}

// ── Folder section ────────────────────────────────────────────────────────

interface RowCommon {
  selectedProjectId: string | null;
  selectedTerminalId: string | null;
  connId: string | null;
  native: OkenaNative;
  onSelectProject: (project: ProjectInfo) => void;
  onSelectTerminal: (project: ProjectInfo, terminalId: string) => void;
}

const FolderSection: React.FC<
  RowCommon & {
    folder: FolderInfo;
    projects: ProjectInfo[];
    onLongPressFolder: () => void;
    onLongPressProject: (project: ProjectInfo, index: number) => void;
  }
> = ({ folder, projects, onLongPressFolder, onLongPressProject, ...row }) => {
  const color = folderColor(folder.folderColor);
  return (
    <View>
      <Pressable style={styles.folderHeader} onLongPress={onLongPressFolder}>
        <Text style={[styles.folderIcon, { color }]}>{'▸'}</Text>
        <Text style={[styles.folderName, { color }]}>{folder.name}</Text>
      </Pressable>
      {projects.map((project, index) => (
        <ProjectRow
          key={`folder-${folder.id}-${project.id}`}
          project={project}
          indent
          {...row}
          onLongPress={() => onLongPressProject(project, index)}
        />
      ))}
    </View>
  );
};

// ── Project row (+ inline expansion when selected) ──────────────────────────

const ProjectRow: React.FC<
  RowCommon & {
    project: ProjectInfo;
    indent: boolean;
    onLongPress: () => void;
  }
> = ({
  project,
  indent,
  selectedProjectId,
  selectedTerminalId,
  connId,
  native,
  onSelectProject,
  onSelectTerminal,
  onLongPress,
}) => {
  const isSelected = project.id === selectedProjectId;
  const color = folderColor(project.folderColor);
  const runningServices = project.services.filter((s) => s.status === 'running').length;

  return (
    <View>
      <Pressable
        style={[styles.projectRow, isSelected && styles.projectRowSelected]}
        onPress={() => onSelectProject(project)}
        onLongPress={onLongPress}
      >
        <Text
          style={[
            styles.projectIcon,
            { color: isSelected ? OkenaColors.accent : color },
            indent && styles.indent,
          ]}
        >
          {'▸'}
        </Text>
        <View style={styles.flex}>
          <Text style={styles.projectName} numberOfLines={1}>
            {project.name}
          </Text>
          {project.gitBranch || runningServices > 0 ? (
            <View style={styles.subtitleRow}>
              {project.gitBranch ? (
                <Text style={styles.subtitleText} numberOfLines={1}>
                  {`⎇ ${project.gitBranch}`}
                </Text>
              ) : null}
              {runningServices > 0 ? (
                <Text style={[styles.subtitleText, styles.subtitleRunning]}>
                  {`  ● ${runningServices}`}
                </Text>
              ) : null}
            </View>
          ) : null}
        </View>
      </Pressable>

      {isSelected ? (
        <View>
          {project.terminalIds.map((tid, idx) => {
            const isTerminalSelected = tid === selectedTerminalId;
            const name = project.terminalNames[tid] ?? `Terminal ${idx + 1}`;
            return (
              <Pressable
                key={tid}
                style={styles.terminalRow}
                onPress={() => onSelectTerminal(project, tid)}
              >
                <Text
                  style={[
                    styles.terminalIcon,
                    isTerminalSelected && styles.terminalSelected,
                  ]}
                >
                  {'❯'}
                </Text>
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
                <Pressable
                  hitSlop={8}
                  onPress={() => {
                    if (connId) void native.closeTerminal(connId, project.id, tid);
                  }}
                >
                  <Text style={styles.terminalClose}>{'✕'}</Text>
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
              <Text style={styles.terminalIcon}>{'＋'}</Text>
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
    <Modal visible={visible} transparent animationType="fade" onRequestClose={onClose}>
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
              <Text style={[styles.dialogBtnText, styles.dialogBtnPrimary]}>Add</Text>
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
  <Modal visible={visible} transparent animationType="slide" onRequestClose={onClose}>
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
              {selected ? <Text style={styles.swatchCheck}>{'✓'}</Text> : null}
            </Pressable>
          );
        })}
      </View>
    </View>
  </Modal>
);

// ── Reorder action sheet ────────────────────────────────────────────────────

const ReorderSheet: React.FC<{
  info: { project: ProjectInfo; folderId: string; index: number; total: number } | null;
  onClose: () => void;
  onChangeColor: () => void;
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
          <Pressable style={styles.sheetItem} onPress={() => onMove(index + 1)}>
            <Text style={styles.sheetItemText}>Move Down</Text>
          </Pressable>
        ) : null}
        {index > 0 ? (
          <Pressable style={styles.sheetItem} onPress={() => onMove(0)}>
            <Text style={styles.sheetItemText}>Move to Top</Text>
          </Pressable>
        ) : null}
        {index < total - 1 ? (
          <Pressable style={styles.sheetItem} onPress={() => onMove(total - 1)}>
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
  backdrop: { ...StyleSheet.absoluteFillObject, backgroundColor: 'rgba(0,0,0,0.5)' },
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
  headerTitle: { color: OkenaColors.textPrimary, fontSize: 22, fontWeight: '700' },
  headerSubtitle: { color: OkenaColors.textSecondary, fontSize: 12, marginTop: 4 },
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
  folderIcon: { fontSize: 14, marginRight: 8 },
  folderName: { fontSize: 12, fontWeight: '600', letterSpacing: 0.5 },
  projectRow: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 12,
    paddingHorizontal: 16,
  },
  projectRowSelected: { backgroundColor: OkenaColors.surfaceOverlay },
  projectIcon: { fontSize: 16, marginRight: 12 },
  indent: { marginLeft: 16 },
  projectName: { color: OkenaColors.textPrimary, fontSize: 15 },
  subtitleRow: { flexDirection: 'row', alignItems: 'center', marginTop: 2 },
  subtitleText: { color: OkenaColors.textTertiary, fontSize: 11 },
  subtitleRunning: { color: OkenaColors.success },
  terminalRow: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 8,
    paddingLeft: 56,
    paddingRight: 12,
  },
  terminalIcon: { color: OkenaColors.textSecondary, fontSize: 13, marginRight: 8 },
  terminalName: { color: OkenaColors.textPrimary, fontSize: 14 },
  terminalSelected: { color: OkenaColors.accent },
  terminalClose: { color: OkenaColors.textTertiary, fontSize: 13 },
  divider: { height: StyleSheet.hairlineWidth, backgroundColor: OkenaColors.border },
  footerItem: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingVertical: 14,
    paddingHorizontal: 16,
  },
  footerIcon: { color: OkenaColors.textSecondary, fontSize: 16, width: 28 },
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
  dialogTitle: { color: OkenaColors.textPrimary, fontSize: 18, fontWeight: '600', marginBottom: 16 },
  dialogInput: {
    backgroundColor: OkenaColors.surface,
    borderRadius: 8,
    paddingHorizontal: 12,
    paddingVertical: 10,
    color: OkenaColors.textPrimary,
    marginBottom: 12,
  },
  dialogActions: { flexDirection: 'row', justifyContent: 'flex-end', marginTop: 4 },
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
  sheetTitle: { color: OkenaColors.textPrimary, fontSize: 16, fontWeight: '600', marginBottom: 12 },
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
  swatchCheck: { color: '#ffffff', fontSize: 18, fontWeight: '700' },
});

export default ProjectDrawer;
