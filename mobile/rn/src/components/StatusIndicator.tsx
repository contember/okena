/**
 * StatusIndicator.tsx — colored dot + label reflecting the connection status.
 *
 * Ported from `mobile/lib/src/widgets/status_indicator.dart`. A small,
 * presentational pill: a colored dot followed by a label, on a tinted rounded
 * background. While connecting / pairing it pulses (the dot + label opacity
 * oscillates); when connected the dot gets a soft glow (shadow). Disconnected
 * and error are static.
 *
 * Reused by the pairing screen (and available elsewhere). Stateless w.r.t. the
 * store — takes the `status` as a prop.
 */

import React, { useEffect, useRef } from 'react';
import { Animated, StyleSheet, View } from 'react-native';

import type { ConnectionStatus } from '../native/okena';
import { OkenaColors } from '../theme';

/**
 * Append an 8-bit alpha (0..1) to a `#RRGGBB` or `#RRGGBBAA` hex color, yielding
 * `#RRGGBBAA`. The theme colors are already `#RRGGBBAA` (alpha `ff`); we replace
 * that trailing alpha. Used to derive the faint bg/border tints (Dart used
 * `color.withOpacity(...)`).
 */
function withAlpha(hex: string, alpha: number): string {
  const base = hex.slice(0, 7); // "#RRGGBB"
  const a = Math.round(Math.max(0, Math.min(1, alpha)) * 255)
    .toString(16)
    .padStart(2, '0');
  return `${base}${a}`;
}

/** Map a status to its dot/label color + label text (mirrors the Dart `switch`). */
function describe(status: ConnectionStatus): { color: string; label: string } {
  switch (status.kind) {
    case 'disconnected':
      return { color: OkenaColors.textTertiary, label: 'Disconnected' };
    case 'connecting':
      return { color: OkenaColors.warning, label: 'Connecting' };
    case 'connected':
      return { color: OkenaColors.success, label: 'Connected' };
    case 'pairing':
      // Dart used `accent` (purple) for its distinct "pairing" hue; amber would
      // also satisfy the contract, but we keep accent to match the original.
      return { color: OkenaColors.accent, label: 'Pairing' };
    case 'error':
      return { color: OkenaColors.error, label: `Error: ${status.message}` };
  }
}

export const StatusIndicator: React.FC<{ status: ConnectionStatus }> = ({ status }) => {
  const { color, label } = describe(status);
  const isConnected = status.kind === 'connected';
  const shouldPulse = status.kind === 'connecting' || status.kind === 'pairing';

  // Pulse opacity: 0.4 ⇄ 1.0, mirroring the Dart 1200ms reversing tween. Drives
  // the dot + label opacity (the chip bg/border stay a faint static tint, which
  // reads close to the Dart `withOpacity(0.1*v)` / `0.2*v` at the bright phase).
  const pulse = useRef(new Animated.Value(1)).current;

  useEffect(() => {
    if (shouldPulse) {
      pulse.setValue(0.4);
      const loop = Animated.loop(
        Animated.sequence([
          Animated.timing(pulse, { toValue: 1, duration: 1200, useNativeDriver: true }),
          Animated.timing(pulse, { toValue: 0.4, duration: 1200, useNativeDriver: true }),
        ]),
      );
      loop.start();
      return () => loop.stop();
    }
    pulse.setValue(1); // settle fully opaque (Dart: `_pulseController.value = 1.0`)
    return undefined;
  }, [shouldPulse, pulse]);

  return (
    <View
      style={[
        styles.pill,
        { backgroundColor: withAlpha(color, 0.1), borderColor: withAlpha(color, 0.2) },
      ]}
    >
      <Animated.View
        style={[
          styles.dot,
          { backgroundColor: color, opacity: pulse },
          isConnected && [styles.glow, { shadowColor: color }],
        ]}
      />
      <Animated.Text
        numberOfLines={1}
        ellipsizeMode="tail"
        style={[styles.label, { color, opacity: pulse }]}
      >
        {label}
      </Animated.Text>
    </View>
  );
};

const styles = StyleSheet.create({
  pill: {
    flexDirection: 'row',
    alignItems: 'center',
    alignSelf: 'center',
    paddingHorizontal: 10,
    paddingVertical: 5,
    borderRadius: 20,
    borderWidth: StyleSheet.hairlineWidth,
  },
  glow: {
    shadowOpacity: 0.5,
    shadowRadius: 6,
    shadowOffset: { width: 0, height: 0 },
    elevation: 4,
  },
  dot: {
    width: 6,
    height: 6,
    borderRadius: 3,
    marginRight: 6,
  },
  label: {
    fontSize: 11,
    fontWeight: '500',
    flexShrink: 1,
  },
});

export default StatusIndicator;
