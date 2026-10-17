/**
 * UsageIndicator.tsx — compact Claude / Codex usage in the workspace app bar.
 *
 * One tiny ring per provider (Claude first, Codex second) with the headline
 * window's percentage inside and the provider initial under it (`C`, `X`),
 * colored by `max(abs, pace)` severity. About 56 pt wide, so the project name
 * keeps most of a small phone's bar. Renders nothing when no provider is
 * enabled and signed in.
 *
 * Pace moves with the clock, so the rings re-derive every minute. That tick
 * lives here, not in the screen, so only this small component re-renders.
 */

import React, { useMemo } from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import { indicatorAccessibilityLabel, indicatorItems, type ProviderId } from '../models/usage';
import type { UsageInfo } from '../native/okena';
import { useWorkspaceStore } from '../state/workspaceStore';
import { OkenaColors } from '../theme';
import { UsageRing } from './UsageVisuals';
import { useNow } from './useNow';

const INITIALS: Record<ProviderId, string> = { claude: 'C', codex: 'X' };

export interface UsageIndicatorProps {
  usage: UsageInfo | null;
  onPress: () => void;
}

export const UsageIndicator: React.FC<UsageIndicatorProps> = ({ usage, onPress }) => {
  const now = useNow(60_000, usage !== null);
  const items = useMemo(() => indicatorItems(usage, now), [usage, now]);
  if (items.length === 0) return null;
  return (
    <Pressable
      hitSlop={6}
      onPress={onPress}
      style={({ pressed }) => [styles.root, pressed && styles.pressed]}
      accessibilityRole="button"
      accessibilityLabel={indicatorAccessibilityLabel(items)}
      accessibilityHint="Opens agent usage details"
      testID="usage-indicator"
    >
      {items.map((item) => (
        <View key={item.provider} style={styles.item}>
          <UsageRing pct={item.usedPercent} severity={item.severity} />
          <Text style={styles.initial} allowFontScaling={false}>
            {INITIALS[item.provider]}
          </Text>
        </View>
      ))}
    </Pressable>
  );
};

/** {@link UsageIndicator} over the workspace store's snapshot. */
export const ConnectedUsageIndicator: React.FC<{ onPress: () => void }> = ({ onPress }) => {
  const usage = useWorkspaceStore((s) => s.usage);
  return <UsageIndicator usage={usage} onPress={onPress} />;
};

const styles = StyleSheet.create({
  root: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 4,
    paddingHorizontal: 4,
    paddingVertical: 2,
    borderRadius: 8,
  },
  pressed: { backgroundColor: OkenaColors.surfaceOverlay },
  item: { alignItems: 'center' },
  initial: {
    color: OkenaColors.textTertiary,
    fontSize: 8,
    fontWeight: '700',
    lineHeight: 9,
    marginTop: 1,
  },
});

export default UsageIndicator;
