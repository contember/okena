/**
 * projectActivity.tsx — shared pieces of the activity-ordered project list
 * (drawer + switcher): the frozen row order, the status dot and the tier
 * header.
 */

import React, { useMemo, useRef } from 'react';
import { StyleSheet, Text, View, type StyleProp, type TextStyle } from 'react-native';

import {
  ACTIVITY_TIER_LABELS,
  applySnapshot,
  buildActivityRows,
  buildProjectList,
  manualProjectOrder,
  snapshotRows,
  type ActivityRow,
  type ActivitySnapshot,
  type ActivityTier,
  type ProjectSignals,
} from '../models';
import { useWorkspaceStore } from '../state';
import { OkenaColors } from '../theme';

/**
 * Projects in activity order. While `frozen` (the list is open) positions and
 * tiers stay as they were when it opened, like the desktop's hover-freeze, so
 * a row never moves under the user's finger; project data and status dots
 * keep updating. Worktrees take their parent's color, as in the manual list.
 */
export function useActivityRows(frozen: boolean): ActivityRow[] {
  const projects = useWorkspaceStore((s) => s.projects);
  const folders = useWorkspaceStore((s) => s.folders);
  const projectOrder = useWorkspaceStore((s) => s.projectOrder);
  const signals = useWorkspaceStore((s) => s.projectSignals);

  const manualOrder = useMemo(
    () => manualProjectOrder(buildProjectList(projects, folders, projectOrder)),
    [projects, folders, projectOrder],
  );
  const live = useMemo(() => {
    const byId = new Map(projects.map((p) => [p.id, p]));
    const colored = projects.map((p) => {
      const parent = p.parentProjectId ? byId.get(p.parentProjectId) : undefined;
      return parent ? { ...p, folderColor: parent.folderColor } : p;
    });
    return buildActivityRows(colored, manualOrder, signals);
  }, [projects, manualOrder, signals]);

  const snapshot = useRef<ActivitySnapshot | null>(null);
  if (!frozen) snapshot.current = null;
  else if (snapshot.current === null && live.length > 0) snapshot.current = snapshotRows(live);
  const frozenSnapshot = snapshot.current;

  return useMemo(
    () => (frozenSnapshot ? applySnapshot(frozenSnapshot, live) : live),
    [frozenSnapshot, live],
  );
}

/** Status dot: amber for attention, green for running, nothing otherwise. */
export const ActivityDot: React.FC<{ signals?: ProjectSignals }> = ({ signals }) => {
  if (!signals || (!signals.attention && !signals.running)) return null;
  const attention = signals.attention;
  return (
    <View
      style={[
        styles.dot,
        { backgroundColor: attention ? OkenaColors.warning : OkenaColors.success },
      ]}
      accessibilityLabel={attention ? 'Needs attention' : 'Running'}
    />
  );
};

/** Section header above a tier's first row. */
export const TierHeader: React.FC<{ tier: ActivityTier; style?: StyleProp<TextStyle> }> = ({
  tier,
  style,
}) => (
  <Text style={[styles.header, style]} accessibilityRole="header">
    {ACTIVITY_TIER_LABELS[tier]}
  </Text>
);

const styles = StyleSheet.create({
  dot: { width: 8, height: 8, borderRadius: 4, marginLeft: 8 },
  header: {
    color: OkenaColors.textSecondary,
    fontSize: 11,
    fontWeight: '600',
    letterSpacing: 0.5,
    paddingHorizontal: 16,
    paddingTop: 12,
    paddingBottom: 4,
  },
});
