/**
 * ConnectionDot.tsx — the connection-quality dot of the app bars: green when
 * the server sent anything in the last {@link ACTIVITY_LIVE_S} s, amber up to
 * {@link ACTIVITY_STALE_S} s or while reconnecting, red after that.
 */

import React from 'react';
import { StyleSheet, View } from 'react-native';

import {
  selectIsReconnecting,
  useConnectionStore,
  useWorkspaceStore,
} from '../state';
import { ACTIVITY_LIVE_S, ACTIVITY_STALE_S } from '../state/workspaceStore';
import { OkenaColors } from '../theme';

export const ConnectionDot: React.FC = () => {
  const reconnecting = useConnectionStore(selectIsReconnecting);
  const secondsSinceActivity = useWorkspaceStore((s) => s.secondsSinceActivity);
  const color = reconnecting
    ? OkenaColors.warning
    : secondsSinceActivity < ACTIVITY_LIVE_S
    ? OkenaColors.success
    : secondsSinceActivity < ACTIVITY_STALE_S
    ? OkenaColors.warning
    : OkenaColors.error;
  return <View style={[styles.dot, { backgroundColor: color }]} />;
};

const styles = StyleSheet.create({
  dot: { width: 8, height: 8, borderRadius: 4, marginHorizontal: 6 },
});
