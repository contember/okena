/**
 * ReconnectBanner.tsx — slim bar shown over the workspace while a
 * once-established connection is being re-established.
 *
 * The workspace (cached state + terminals) stays on screen underneath; the
 * banner says what is happening, for how long, and offers "Retry now" (skip the
 * backoff, fresh attempt budget). Also home of {@link useElapsedSeconds}, shared
 * with the pairing screen's connecting state.
 */

import React, { useEffect, useState } from 'react';
import { ActivityIndicator, Pressable, StyleSheet, Text, View } from 'react-native';

import type { ConnectionStatus } from '../native/okena';
import { OkenaColors } from '../theme';
import { withAlpha } from './StatusIndicator';

/** Whole seconds since `since` (a `Date.now()` value), ticking every second. */
export function useElapsedSeconds(since: number | null): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (since === null) return undefined;
    setNow(Date.now());
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [since]);
  return since === null ? 0 : Math.max(0, Math.floor((now - since) / 1000));
}

/** Banner text for a reconnecting status, e.g. `Reconnecting… · attempt 2 · 7s`. */
export function reconnectLabel(status: ConnectionStatus, elapsed: number): string {
  const parts = ['Reconnecting…'];
  if (status.kind === 'reconnecting') parts.push(`attempt ${status.attempt}`);
  if (elapsed > 0) parts.push(`${elapsed}s`);
  return parts.join(' · ');
}

export const ReconnectBanner: React.FC<{
  status: ConnectionStatus;
  since: number | null;
  onRetry: () => void;
}> = ({ status, since, onRetry }) => {
  const elapsed = useElapsedSeconds(since);
  return (
    <View style={styles.bar} accessibilityRole="alert">
      <ActivityIndicator size="small" color={OkenaColors.warning} style={styles.spinner} />
      <Text style={styles.label} numberOfLines={1}>
        {reconnectLabel(status, elapsed)}
      </Text>
      <Pressable
        hitSlop={8}
        onPress={onRetry}
        style={({ pressed }) => [styles.retry, pressed && styles.retryPressed]}
        accessibilityLabel="Retry now"
      >
        <Text style={styles.retryText}>Retry now</Text>
      </Pressable>
    </View>
  );
};

const styles = StyleSheet.create({
  bar: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 12,
    paddingVertical: 6,
    backgroundColor: withAlpha(OkenaColors.warning, 0.1),
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: withAlpha(OkenaColors.warning, 0.2),
  },
  spinner: { transform: [{ scale: 0.7 }], marginRight: 6 },
  label: { flex: 1, color: OkenaColors.warning, fontSize: 12, fontWeight: '500' },
  retry: {
    paddingHorizontal: 10,
    paddingVertical: 3,
    borderRadius: 12,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: withAlpha(OkenaColors.warning, 0.5),
  },
  retryPressed: { opacity: 0.6 },
  retryText: { color: OkenaColors.warning, fontSize: 12, fontWeight: '600' },
});

export default ReconnectBanner;
