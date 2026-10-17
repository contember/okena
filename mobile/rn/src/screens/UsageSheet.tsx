/**
 * UsageSheet.tsx — Claude Code / Codex usage of the connected server.
 *
 * Opens from the app-bar usage indicator and from the drawer footer. Same
 * modal sheet style as {@link SettingsSheet}. The header names the server,
 * because the numbers are the accounts signed in on that machine.
 *
 * Per provider: plan badge, status or error, and one row per window as the
 * desktop popover shows it (bar with the time-elapsed tick, used %, pace note,
 * "resets in …", history sparkline), then Claude extra usage or Codex credits.
 *
 * Opening the sheet asks the daemon to refetch; a spinner shows until the new
 * snapshot arrives or a few seconds pass (the daemon throttles to once a
 * minute per provider, so a reopen often gets no new fetch).
 */

import React, { useEffect, useMemo } from 'react';
import {
  ActivityIndicator,
  Modal,
  Platform,
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import type { UsageInfo } from '../native/okena';
import { savedServerDisplayName } from '../models/savedServer';
import { useConnectionStore, useWorkspaceStore } from '../state';
import {
  buildUsageView,
  type ProviderView,
  type UsageWindowView,
} from '../models/usage';
import { OkenaColors, OkenaTypography } from '../theme';
import { UsageBar, UsageSparkline, severityColor } from '../components/UsageVisuals';
import { useNow } from '../components/useNow';

export interface UsageSheetProps {
  visible: boolean;
  onClose: () => void;
  /** Display name of the connected server. */
  serverName: string;
  usage: UsageInfo | null;
  refreshing: boolean;
  /** Called each time the sheet opens. */
  onRefresh: () => void;
}

export const UsageSheet: React.FC<UsageSheetProps> = ({
  visible,
  onClose,
  serverName,
  usage,
  refreshing,
  onRefresh,
}) => {
  const insets = useSafeAreaInsets();
  const pageSheet = Platform.OS === 'ios';
  const now = useNow(15_000, visible);

  useEffect(() => {
    if (visible) onRefresh();
    // Refresh on open only; `onRefresh` identity is not a reason to refetch.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [visible]);

  const providers = useMemo(() => (usage ? buildUsageView(usage, now) : []), [usage, now]);

  return (
    <Modal
      visible={visible}
      animationType="slide"
      presentationStyle={pageSheet ? 'pageSheet' : 'fullScreen'}
      onRequestClose={onClose}
    >
      <View style={[styles.root, !pageSheet && { paddingTop: insets.top }]}>
        <View style={styles.header}>
          <View style={styles.headerText}>
            <View style={styles.titleRow}>
              <Text style={styles.title}>Usage</Text>
              {refreshing ? (
                <ActivityIndicator
                  size="small"
                  color={OkenaColors.textTertiary}
                  style={styles.spinner}
                  accessibilityLabel="Refreshing usage"
                />
              ) : null}
            </View>
            <Text style={styles.subtitle} numberOfLines={1}>
              {`Accounts signed in on ${serverName}`}
            </Text>
          </View>
          <Pressable
            hitSlop={8}
            style={({ pressed }) => [styles.done, pressed && styles.pressed]}
            onPress={onClose}
            accessibilityRole="button"
          >
            <Text style={styles.doneText}>Done</Text>
          </Pressable>
        </View>

        <ScrollView contentContainerStyle={{ paddingBottom: insets.bottom + 24 }}>
          {providers.length === 0 ? (
            <Text style={styles.empty}>
              {usage
                ? `Claude Code and Codex usage are turned off on ${serverName}. Enable the Claude or Codex extension in Okena's settings there.`
                : `No usage data from ${serverName} yet. Okena on that machine may need an update.`}
            </Text>
          ) : null}
          {providers.map((p, i) => (
            <ProviderSection key={p.provider} provider={p} first={i === 0} />
          ))}
        </ScrollView>
      </View>
    </Modal>
  );
};

/** {@link UsageSheet} over the stores: the active server's snapshot and refresh. */
export const ConnectedUsageSheet: React.FC<{ visible: boolean; onClose: () => void }> = ({
  visible,
  onClose,
}) => {
  const usage = useWorkspaceStore((s) => s.usage);
  const refreshing = useWorkspaceStore((s) => s.usageRefreshing);
  const refreshUsage = useWorkspaceStore((s) => s.refreshUsage);
  const server = useConnectionStore((s) => s.activeServer);
  return (
    <UsageSheet
      visible={visible}
      onClose={onClose}
      serverName={server ? savedServerDisplayName(server) : 'this server'}
      usage={usage}
      refreshing={refreshing}
      onRefresh={() => refreshUsage()}
    />
  );
};

const ProviderSection: React.FC<{ provider: ProviderView; first: boolean }> = ({ provider: p, first }) => (
  <View testID={`usage-provider-${p.provider}`}>
    <View style={[styles.sectionHeader, !first && styles.sectionSpacing]}>
      <Text style={styles.sectionLabel}>{p.name.toUpperCase()}</Text>
      {p.plan ? (
        <View style={styles.planBadge}>
          <Text style={styles.planText}>{p.plan}</Text>
        </View>
      ) : null}
      {p.fetching ? <Text style={styles.fetching}>Updating…</Text> : null}
    </View>
    <View style={styles.group}>
      {p.statusText ? (
        <View style={[styles.row, styles.statusRow]}>
          <Text style={[styles.statusText, p.isError && styles.statusError]}>{p.statusText}</Text>
        </View>
      ) : null}
      {p.windows.map((w, i) => (
        <WindowRow key={w.id} window={w} last={i === p.windows.length - 1 && !hasExtras(p)} />
      ))}
      {p.extra ? (
        <View style={styles.row}>
          <View style={styles.kvRow}>
            <Text style={styles.rowTitle}>Extra usage</Text>
            <Text style={styles.kvValue}>{`${p.extra.used} / ${p.extra.limit}`}</Text>
          </View>
          <UsageBar usedPercent={p.extra.utilization} segments={NO_SEGMENTS} height={6} />
        </View>
      ) : null}
      {p.credits ? (
        <View style={styles.row}>
          <View style={styles.kvRow}>
            <Text style={styles.rowTitle}>Credits</Text>
            <Text style={[styles.kvValue, p.credits.unlimited && { color: OkenaColors.success }]}>
              {p.credits.text}
            </Text>
          </View>
        </View>
      ) : null}
      {p.resetCredits ? (
        <View style={styles.row}>
          <View style={styles.kvRow}>
            <Text style={styles.rowTitle}>{p.resetCredits.summary}</Text>
            {p.resetCredits.firstExpiry ? (
              <Text style={styles.meta}>{p.resetCredits.firstExpiry}</Text>
            ) : null}
          </View>
        </View>
      ) : null}
      {p.windows.length === 0 && !p.statusText && !hasExtras(p) ? (
        <View style={[styles.row, styles.statusRow]}>
          <Text style={styles.statusText}>No limits reported.</Text>
        </View>
      ) : null}
    </View>
    {p.detailText ? <Text style={styles.footnote}>{p.detailText}</Text> : null}
  </View>
);

const NO_SEGMENTS = { dividers: [], workingDates: [] };

function hasExtras(p: ProviderView): boolean {
  return p.extra !== undefined || p.credits !== undefined || p.resetCredits !== undefined;
}

const PACE_NOTES = {
  warning: 'Ahead of pace',
  critical: 'Slow down to last the period',
} as const;

function windowAccessibilityLabel(w: UsageWindowView): string {
  const parts = [`${w.title}, ${w.period} window`, `${Math.round(w.usedPercent)}% used`];
  if (w.timePct !== undefined) parts.push(`${Math.round(w.timePct)}% of the time elapsed`);
  if (w.pace !== 'normal') parts.push(PACE_NOTES[w.pace]);
  if (w.resetsIn) parts.push(w.resetsIn);
  return parts.join(', ');
}

const WindowRow: React.FC<{ window: UsageWindowView; last: boolean }> = ({ window: w, last }) => {
  const paceNote = w.pace === 'normal' ? undefined : PACE_NOTES[w.pace];
  return (
    <View
      style={[styles.row, !last && styles.rowDivider]}
      accessible
      accessibilityLabel={windowAccessibilityLabel(w)}
      testID={`usage-window-${w.id}`}
    >
      <View style={styles.windowHeader}>
        <Text style={styles.rowTitle} numberOfLines={1}>
          {w.title}
        </Text>
        <Text style={styles.period}>{w.period}</Text>
        <View style={styles.flexSpacer} />
        <Text style={[styles.pct, { color: severityColor(w.severity) }]}>
          {`${Math.round(w.usedPercent)}%`}
        </Text>
      </View>
      <UsageBar usedPercent={w.usedPercent} timePct={w.timePct} segments={w.segments} />
      {w.history.length >= 2 ? (
        <View style={styles.spark}>
          <UsageSparkline points={w.history} severity={w.severity} />
        </View>
      ) : null}
      {paceNote || w.resetsIn ? (
        <View style={styles.windowFooter}>
          <Text style={[styles.paceNote, { color: severityColor(w.pace) }]} numberOfLines={1}>
            {paceNote ?? ''}
          </Text>
          {w.resetsIn ? (
            <Text style={styles.meta} numberOfLines={1}>
              {w.resetClock ? `${w.resetsIn} · ${w.resetClock}` : w.resetsIn}
            </Text>
          ) : null}
        </View>
      ) : null}
    </View>
  );
};

const styles = StyleSheet.create({
  root: { flex: 1, backgroundColor: OkenaColors.background },
  header: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    paddingHorizontal: 20,
    paddingTop: 16,
    paddingBottom: 12,
  },
  headerText: { flex: 1, paddingRight: 12 },
  titleRow: { flexDirection: 'row', alignItems: 'center' },
  title: { ...OkenaTypography.title },
  spinner: { marginLeft: 10 },
  subtitle: { color: OkenaColors.textSecondary, fontSize: 13, marginTop: 2 },
  done: { minHeight: 44, justifyContent: 'center', paddingHorizontal: 4 },
  doneText: { color: OkenaColors.accent, fontSize: 17, fontWeight: '600' },
  pressed: { opacity: 0.6 },
  empty: {
    color: OkenaColors.textSecondary,
    fontSize: 14,
    marginHorizontal: 32,
    marginTop: 24,
    lineHeight: 20,
  },
  sectionHeader: {
    flexDirection: 'row',
    alignItems: 'center',
    marginHorizontal: 32,
    marginTop: 12,
    marginBottom: 6,
  },
  sectionSpacing: { marginTop: 28 },
  sectionLabel: {
    color: OkenaColors.textTertiary,
    fontSize: 12,
    fontWeight: '600',
    letterSpacing: 0.6,
  },
  planBadge: {
    marginLeft: 8,
    paddingHorizontal: 6,
    paddingVertical: 1,
    borderRadius: 4,
    backgroundColor: OkenaColors.accentSoft,
  },
  planText: { color: OkenaColors.accent, fontSize: 11, fontWeight: '600' },
  fetching: { marginLeft: 'auto', color: OkenaColors.textTertiary, fontSize: 12 },
  group: {
    marginHorizontal: 16,
    borderRadius: 12,
    backgroundColor: OkenaColors.surfaceElevated,
    overflow: 'hidden',
  },
  row: { paddingHorizontal: 16, paddingVertical: 12, gap: 6 },
  rowDivider: {
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.borderLight,
  },
  statusRow: { paddingVertical: 10 },
  statusText: { color: OkenaColors.textSecondary, fontSize: 13 },
  statusError: { color: OkenaColors.error },
  windowHeader: { flexDirection: 'row', alignItems: 'baseline' },
  rowTitle: { color: OkenaColors.textPrimary, fontSize: 15, flexShrink: 1 },
  period: { color: OkenaColors.textTertiary, fontSize: 12, marginLeft: 6 },
  flexSpacer: { flex: 1 },
  pct: { fontSize: 17, fontWeight: '600', fontVariant: ['tabular-nums'] },
  spark: { marginTop: 2 },
  windowFooter: { flexDirection: 'row', justifyContent: 'space-between', gap: 8 },
  paceNote: { fontSize: 12, fontWeight: '500', flexShrink: 1 },
  meta: { color: OkenaColors.textTertiary, fontSize: 12 },
  kvRow: { flexDirection: 'row', alignItems: 'baseline', justifyContent: 'space-between', gap: 8 },
  kvValue: { color: OkenaColors.textPrimary, fontSize: 15, fontVariant: ['tabular-nums'] },
  footnote: {
    color: OkenaColors.textTertiary,
    fontSize: 12,
    marginHorizontal: 32,
    marginTop: 8,
  },
});

export default UsageSheet;
