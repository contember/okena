/**
 * DashboardScreen.tsx — the first screen of a connection: what needs the user
 * and the pinned projects, before any one terminal.
 *
 *   - "Needs you" lists every terminal with an open Claude dialog, a blocked
 *     agent, an unseen notification or bell, or an agent that finished since
 *     the phone last showed it (`attention` in the workspace store). A Claude
 *     dialog shows what it asks ({@link useClaudeDialogsStore}) and opens the
 *     {@link ClaudeDialogSheet}; any other row opens its terminal.
 *   - "Pinned projects" lists the projects pinned on the desktop with branch,
 *     line changes and status; a row opens the project.
 *
 * The app bar's Terminals button opens the workspace on the current
 * selection; the workspace's dashboard button comes back.
 */

import React, { useCallback, useEffect, useMemo, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';

import { ConnectionDot } from '../components/ConnectionDot';
import { ClaudeDialogSheet } from '../components/ClaudeDialogSheet';
import { DialogBadge } from '../components/ClaudeIndicator';
import { Icon, type IconName } from '../components/Icon';
import { folderColor } from '../components/ProjectDrawer';
import { useActivityRows } from '../components/projectActivity';
import { ReconnectBanner } from '../components/ReconnectBanner';
import { withAlpha } from '../components/StatusIndicator';
import {
  projectDisplayName,
  savedServerDisplayName,
  terminalLabel,
  type AttentionItem,
  type AttentionKind,
  type ProjectSignals,
} from '../models';
import { summarizeClaudeDialog, type ApiClaudeDialog } from '../native/claude';
import type { ProjectInfo } from '../native/okena';
import {
  selectIsReconnecting,
  useClaudeDialogsStore,
  useConnectionStore,
  useWorkspaceStore,
} from '../state';
import { OkenaColors } from '../theme';

const KIND_ICON: Record<AttentionKind, IconName> = {
  dialog: 'chat',
  blocked: 'prompt',
  notification: 'bell',
  bell: 'bell',
  done: 'check',
};

export const DashboardScreen: React.FC = () => {
  const connId = useConnectionStore((s) => s.connId);
  const activeServer = useConnectionStore((s) => s.activeServer);
  const connStatus = useConnectionStore((s) => s.status);
  const reconnecting = useConnectionStore(selectIsReconnecting);
  const connectingSince = useConnectionStore((s) => s.connectingSince);
  const retryNow = useConnectionStore((s) => s.retryNow);

  const projects = useWorkspaceStore((s) => s.projects);
  const attention = useWorkspaceStore((s) => s.attention);
  const signals = useWorkspaceStore((s) => s.projectSignals);
  const showWorkspace = useWorkspaceStore((s) => s.showWorkspace);
  const setTerminalView = useWorkspaceStore((s) => s.setTerminalView);

  const dialogs = useClaudeDialogsStore((s) => s.dialogs);
  const openDialogs = useClaudeDialogsStore((s) => s.open);
  const closeDialogs = useClaudeDialogsStore((s) => s.close);

  const [sheetItem, setSheetItem] = useState<AttentionItem | null>(null);
  const closeSheet = useCallback(() => setSheetItem(null), []);

  useEffect(() => {
    if (connId === null) return;
    openDialogs(connId);
    return closeDialogs;
  }, [connId, openDialogs, closeDialogs]);

  const projectById = useMemo(() => new Map(projects.map((p) => [p.id, p])), [projects]);
  const pinned = useActivityRows(false).filter((row) => row.tier === 'pinned');
  const sheetProject = sheetItem ? projectById.get(sheetItem.projectId) : undefined;

  const openItem = (item: AttentionItem) => {
    if (item.kind === 'dialog') setSheetItem(item);
    else showWorkspace(item.projectId, item.terminalId);
  };

  return (
    <View style={styles.root}>
      <View style={styles.appBar}>
        <View style={styles.titleBlock}>
          <Text style={styles.title} accessibilityRole="header">
            Dashboard
          </Text>
          <View style={styles.serverRow}>
            <ConnectionDot />
            <Text style={styles.serverName} numberOfLines={1}>
              {activeServer ? savedServerDisplayName(activeServer) : ''}
            </Text>
          </View>
        </View>
        <Pressable
          style={styles.terminalsButton}
          onPress={() => showWorkspace()}
          accessibilityRole="button"
        >
          <Icon name="prompt" size={18} color={OkenaColors.textPrimary} />
          <Text style={styles.terminalsButtonText}>Terminals</Text>
        </Pressable>
      </View>

      {reconnecting ? (
        <ReconnectBanner status={connStatus} since={connectingSince} onRetry={retryNow} />
      ) : null}

      <ScrollView contentContainerStyle={styles.content}>
        <SectionHeader
          label={`Needs you · ${attention.length}`}
          color={attention.length > 0 ? OkenaColors.warning : OkenaColors.textSecondary}
        />
        {attention.length === 0 ? (
          <View style={[styles.card, styles.allClear]}>
            <Icon name="check" size={20} color={OkenaColors.success} />
            <View style={styles.rowText}>
              <Text style={styles.rowTitle}>All clear</Text>
              <Text style={styles.rowSubtitle}>
                No Claude questions, bells or finished agents.
              </Text>
            </View>
          </View>
        ) : (
          <View style={styles.card}>
            {attention.map((item, index) => {
              const project = projectById.get(item.projectId);
              if (!project) return null;
              return (
                <AttentionRow
                  key={item.terminalId}
                  item={item}
                  project={project}
                  summary={attentionSummary(item, project, dialogs[item.terminalId])}
                  divider={index > 0}
                  onPress={() => openItem(item)}
                />
              );
            })}
          </View>
        )}

        <SectionHeader label={`Pinned projects · ${pinned.length}`} />
        {pinned.length === 0 ? (
          <Text style={styles.hint}>Pin a project on the desktop to see it here.</Text>
        ) : (
          <View style={styles.card}>
            {pinned.map((row, index) => (
              <PinnedRow
                key={row.project.id}
                project={row.project}
                signals={signals[row.project.id]}
                divider={index > 0}
                onPress={() => showWorkspace(row.project.id)}
              />
            ))}
          </View>
        )}
      </ScrollView>

      {connId !== null && sheetItem !== null && sheetProject ? (
        <ClaudeDialogSheet
          connId={connId}
          terminalId={sheetItem.terminalId}
          title={`${projectDisplayName(sheetProject)} · ${terminalLabel(
            sheetProject,
            sheetItem.terminalId,
          )}`}
          onClose={closeSheet}
          onOpenChat={() => {
            setTerminalView(sheetItem.terminalId, 'chat');
            setSheetItem(null);
            showWorkspace(sheetItem.projectId, sheetItem.terminalId);
          }}
        />
      ) : null}
    </View>
  );
};

/** The row's second line: what the dialog asks, or why the terminal needs the user. */
function attentionSummary(
  item: AttentionItem,
  project: ProjectInfo,
  dialogs: readonly ApiClaudeDialog[] | undefined,
): string {
  const custom = project.terminalAgentStatus[item.terminalId]?.custom;
  switch (item.kind) {
    case 'dialog': {
      const first = dialogs?.[0];
      return first ? summarizeClaudeDialog(first) : 'Claude is waiting for you';
    }
    case 'blocked':
      return custom ?? 'Agent is blocked';
    case 'notification':
      return 'Notification';
    case 'bell':
      return 'Bell';
    case 'done':
      return custom ?? 'Agent finished';
  }
}

const SectionHeader: React.FC<{ label: string; color?: string }> = ({
  label,
  color = OkenaColors.textSecondary,
}) => (
  <Text style={[styles.sectionHeader, { color }]} accessibilityRole="header">
    {label.toUpperCase()}
  </Text>
);

const AttentionRow: React.FC<{
  item: AttentionItem;
  project: ProjectInfo;
  summary: string;
  divider: boolean;
  onPress: () => void;
}> = ({ item, project, summary, divider, onPress }) => {
  const urgent = item.kind !== 'done';
  const name = `${projectDisplayName(project)} · ${terminalLabel(project, item.terminalId)}`;
  return (
    <Pressable
      style={[styles.row, divider && styles.rowDivider]}
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={`${name}: ${summary}`}
    >
      <View style={[styles.kindIcon, urgent && styles.kindIconUrgent]}>
        <Icon
          name={KIND_ICON[item.kind]}
          size={16}
          color={urgent ? OkenaColors.warning : OkenaColors.success}
        />
      </View>
      <View style={styles.rowText}>
        <Text style={styles.rowMeta} numberOfLines={1}>
          {name}
        </Text>
        <Text style={styles.rowTitle} numberOfLines={1}>
          {summary}
        </Text>
      </View>
      {item.dialogs > 1 ? <DialogBadge count={item.dialogs} /> : null}
      <Icon name="chevronRight" size={16} color={OkenaColors.textTertiary} />
    </Pressable>
  );
};

const PinnedRow: React.FC<{
  project: ProjectInfo;
  signals: ProjectSignals | undefined;
  divider: boolean;
  onPress: () => void;
}> = ({ project, signals, divider, onPress }) => {
  const status = signals?.attention
    ? { label: 'needs you', color: OkenaColors.warning }
    : signals?.running
    ? { label: 'running', color: OkenaColors.success }
    : null;
  const terminals = project.terminalIds.length;
  return (
    <Pressable
      style={[styles.row, divider && styles.rowDivider]}
      onPress={onPress}
      accessibilityRole="button"
    >
      <View style={[styles.projectDot, { backgroundColor: folderColor(project.folderColor) }]} />
      <View style={styles.rowText}>
        <Text style={styles.projectName} numberOfLines={1}>
          {projectDisplayName(project)}
        </Text>
        {project.gitBranch ? (
          <View style={styles.branchRow}>
            <Icon name="gitBranch" size={12} color={OkenaColors.textSecondary} />
            <Text style={styles.branch} numberOfLines={1}>
              {project.gitBranch}
            </Text>
            {project.gitLinesAdded > 0 ? (
              <Text style={styles.added}>{`+${project.gitLinesAdded}`}</Text>
            ) : null}
            {project.gitLinesRemoved > 0 ? (
              <Text style={styles.removed}>{`−${project.gitLinesRemoved}`}</Text>
            ) : null}
          </View>
        ) : null}
      </View>
      <View style={styles.projectSide}>
        {status ? (
          <View style={styles.statusRow}>
            <View style={[styles.statusDot, { backgroundColor: status.color }]} />
            <Text style={[styles.statusText, { color: status.color }]}>{status.label}</Text>
          </View>
        ) : null}
        <Text style={styles.rowMeta}>
          {terminals === 1 ? '1 terminal' : `${terminals} terminals`}
        </Text>
      </View>
    </Pressable>
  );
};

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: OkenaColors.background },
  appBar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 16,
    paddingVertical: 10,
    backgroundColor: OkenaColors.surface,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
  titleBlock: { flex: 1, minWidth: 0 },
  title: { color: OkenaColors.textPrimary, fontSize: 22, fontWeight: '700', letterSpacing: -0.3 },
  serverRow: { flexDirection: 'row', alignItems: 'center', marginTop: 2, marginLeft: -6 },
  serverName: { flexShrink: 1, color: OkenaColors.textSecondary, fontSize: 12 },
  terminalsButton: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
    minHeight: 44,
    paddingHorizontal: 12,
    borderRadius: 12,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  terminalsButtonText: { color: OkenaColors.textPrimary, fontSize: 14, fontWeight: '500' },
  content: { padding: 16, paddingBottom: 32 },
  sectionHeader: {
    fontSize: 12,
    fontWeight: '600',
    letterSpacing: 0.8,
    marginTop: 8,
    marginBottom: 8,
  },
  card: {
    marginBottom: 16,
    borderRadius: 14,
    overflow: 'hidden',
    backgroundColor: OkenaColors.surfaceElevated,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.border,
  },
  allClear: { flexDirection: 'row', alignItems: 'center', gap: 12, padding: 16 },
  hint: { color: OkenaColors.textSecondary, fontSize: 13, marginBottom: 16 },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 12,
    minHeight: 60,
    paddingHorizontal: 12,
    paddingVertical: 8,
  },
  rowDivider: {
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopColor: OkenaColors.border,
  },
  rowText: { flex: 1, minWidth: 0, gap: 2 },
  rowMeta: { color: OkenaColors.textSecondary, fontSize: 12 },
  rowTitle: { color: OkenaColors.textPrimary, fontSize: 14, fontWeight: '500' },
  rowSubtitle: { color: OkenaColors.textSecondary, fontSize: 13 },
  kindIcon: {
    width: 32,
    height: 32,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: OkenaColors.surfaceOverlay,
  },
  kindIconUrgent: { backgroundColor: withAlpha(OkenaColors.warning, 0.14) },
  projectDot: { width: 10, height: 10, borderRadius: 5 },
  projectName: { color: OkenaColors.textPrimary, fontSize: 15, fontWeight: '600' },
  branchRow: { flexDirection: 'row', alignItems: 'center', gap: 6, minWidth: 0 },
  branch: { flexShrink: 1, color: OkenaColors.textSecondary, fontSize: 12 },
  added: { color: OkenaColors.success, fontSize: 12 },
  removed: { color: OkenaColors.error, fontSize: 12 },
  projectSide: { alignItems: 'flex-end', gap: 3 },
  statusRow: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  statusDot: { width: 7, height: 7, borderRadius: 4 },
  statusText: { fontSize: 12 },
});

export default DashboardScreen;
