/**
 * DataUsage.tsx — what the current connection has downloaded, by kind.
 *
 * Reads `getTrafficStats` (payload bytes counted in the Rust transport since
 * the connection was created) every couple of seconds while mounted. TLS,
 * HTTP and WebSocket framing are not included, so the system's per-app
 * counter reads somewhat higher.
 */

import React, { useEffect, useState } from 'react';
import { StyleSheet, Text, View } from 'react-native';

import type { ConnId, OkenaNative, TrafficStats } from '../native/okena';
import { OkenaColors } from '../theme';

export const DATA_USAGE_POLL_MS = 2000;

export type TrafficNative = Pick<OkenaNative, 'getTrafficStats'>;

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

export interface TrafficRow {
  label: string;
  value: string;
}

/** The rows shown for `stats`, total last. */
export function trafficRows(stats: TrafficStats): TrafficRow[] {
  const total =
    stats.ptyBytes +
    stats.snapshotBytes +
    stats.stateBytes +
    stats.settingsBytes +
    stats.eventBytes;
  const fetches = stats.stateFetches === 1 ? '1 fetch' : `${stats.stateFetches} fetches`;
  return [
    { label: 'Terminal output', value: formatBytes(stats.ptyBytes) },
    { label: 'Terminal snapshots', value: formatBytes(stats.snapshotBytes) },
    {
      label: 'Workspace state',
      value: `${formatBytes(stats.stateBytes)} · ${fetches}`,
    },
    {
      label: 'Events and settings',
      value: formatBytes(stats.eventBytes + stats.settingsBytes),
    },
    { label: 'Total', value: formatBytes(total) },
  ];
}

export const DataUsageGroup: React.FC<{ native: TrafficNative; connId: ConnId }> = ({
  native,
  connId,
}) => {
  const [stats, setStats] = useState<TrafficStats | undefined>(() =>
    native.getTrafficStats(connId),
  );
  useEffect(() => {
    setStats(native.getTrafficStats(connId));
    const timer = setInterval(
      () => setStats(native.getTrafficStats(connId)),
      DATA_USAGE_POLL_MS,
    );
    return () => clearInterval(timer);
  }, [native, connId]);

  if (!stats) return null;
  const rows = trafficRows(stats);
  return (
    <View style={styles.group}>
      {rows.map((row, i) => (
        <View
          key={row.label}
          style={[styles.row, i < rows.length - 1 && styles.rowDivider]}
          accessible
          accessibilityLabel={`${row.label}: ${row.value}`}
        >
          <Text style={[styles.label, i === rows.length - 1 && styles.total]}>
            {row.label}
          </Text>
          <Text style={[styles.value, i === rows.length - 1 && styles.total]}>
            {row.value}
          </Text>
        </View>
      ))}
    </View>
  );
};

const styles = StyleSheet.create({
  group: {
    marginHorizontal: 16,
    borderRadius: 12,
    backgroundColor: OkenaColors.surfaceElevated,
    overflow: 'hidden',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    justifyContent: 'space-between',
    minHeight: 44,
    paddingHorizontal: 16,
    paddingVertical: 8,
  },
  rowDivider: {
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
  label: { color: OkenaColors.textPrimary, fontSize: 15 },
  value: { color: OkenaColors.textSecondary, fontSize: 15, fontVariant: ['tabular-nums'] },
  total: { fontWeight: '600', color: OkenaColors.textPrimary },
});
