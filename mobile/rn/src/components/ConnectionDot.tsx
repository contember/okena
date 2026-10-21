/**
 * ConnectionDot.tsx — the connection-quality dot of the app bars: green when
 * the server sent data in the last 3 s, amber up to 10 s or while
 * reconnecting, red after that.
 */

import React from 'react';
import { StyleSheet, View } from 'react-native';

import {
  selectIsReconnecting,
  useConnectionStore,
  useWorkspaceStore,
} from '../state';
import { OkenaColors } from '../theme';

export const ConnectionDot: React.FC = () => {
  const reconnecting = useConnectionStore(selectIsReconnecting);
  const secondsSinceActivity = useWorkspaceStore((s) => s.secondsSinceActivity);
  const color = reconnecting
    ? OkenaColors.warning
    : secondsSinceActivity < 3
    ? OkenaColors.success
    : secondsSinceActivity < 10
    ? OkenaColors.warning
    : OkenaColors.error;
  return <View style={[styles.dot, { backgroundColor: color }]} />;
};

const styles = StyleSheet.create({
  dot: { width: 8, height: 8, borderRadius: 4, marginHorizontal: 6 },
});
