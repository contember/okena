/**
 * ClaudeIndicator.tsx — the agent lifecycle dot and the open-dialog badge of a
 * terminal, and both as a tappable "Chat" pill that opens its Claude chat.
 */

import React from 'react';
import { Pressable, StyleSheet, Text, View } from 'react-native';

import type { AgentLifecycle } from '../native/claude';
import { OkenaColors } from '../theme';

export function lifecycleColor(lifecycle: AgentLifecycle | undefined): string {
  switch (lifecycle) {
    case 'working':
      return OkenaColors.accent;
    case 'blocked':
      return OkenaColors.warning;
    case 'done':
      return OkenaColors.success;
    case 'idle':
    case undefined:
      return OkenaColors.textTertiary;
  }
}

export const LifecycleDot: React.FC<{
  lifecycle: AgentLifecycle | undefined;
}> = ({ lifecycle }) => (
  <View style={[styles.dot, { backgroundColor: lifecycleColor(lifecycle) }]} />
);

export const DialogBadge: React.FC<{ count: number }> = ({ count }) => (
  <View style={styles.badge}>
    <Text style={styles.badgeText}>{count}</Text>
  </View>
);

export const ClaudeIndicator: React.FC<{
  lifecycle: AgentLifecycle | undefined;
  dialogs: number;
  onPress: () => void;
}> = ({ lifecycle, dialogs, onPress }) => (
  <Pressable
    hitSlop={8}
    style={styles.pill}
    onPress={onPress}
    accessibilityRole="button"
    accessibilityLabel={
      dialogs > 0 ? `Open Claude chat, ${dialogs} open dialogs` : 'Open Claude chat'
    }
  >
    <LifecycleDot lifecycle={lifecycle} />
    <Text style={styles.label}>Chat</Text>
    {dialogs > 0 ? <DialogBadge count={dialogs} /> : null}
  </Pressable>
);

const styles = StyleSheet.create({
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    paddingHorizontal: 10,
    paddingVertical: 5,
    borderRadius: 12,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
    marginHorizontal: 6,
  },
  // The ring keeps the dot visible on an accent-colored chip.
  dot: {
    width: 9,
    height: 9,
    borderRadius: 4.5,
    borderWidth: 1.5,
    borderColor: OkenaColors.surface,
  },
  label: { color: OkenaColors.textSecondary, fontSize: 12, marginLeft: 5 },
  badge: {
    minWidth: 16,
    height: 16,
    borderRadius: 8,
    paddingHorizontal: 4,
    marginLeft: 5,
    alignItems: 'center',
    justifyContent: 'center',
    backgroundColor: OkenaColors.warning,
  },
  badgeText: { color: OkenaColors.background, fontSize: 10, fontWeight: '700' },
});
