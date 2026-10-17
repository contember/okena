/**
 * UsageVisuals.tsx — drawing primitives for agent usage: the app-bar ring,
 * the window bar with its pace marker, and the history sparkline.
 *
 * Severity colors follow the desktop (`okena-usage::severity_color`): its
 * metric green / yellow / red map to the app's success / warning / error.
 */

import React, { useMemo, useState } from 'react';
import { StyleSheet, Text, View, type LayoutChangeEvent } from 'react-native';
import { Canvas, Path, Skia } from '@shopify/react-native-skia';

import { absSeverity, type Segments, type Severity } from '../models/usage';
import { OkenaColors } from '../theme';

export const SEVERITY_COLORS: Record<Severity, string> = {
  normal: OkenaColors.success,
  warning: OkenaColors.warning,
  critical: OkenaColors.error,
};

export function severityColor(severity: Severity): string {
  return SEVERITY_COLORS[severity];
}

const clampPct = (pct: number) => Math.min(Math.max(pct, 0), 100);

// ── Ring ─────────────────────────────────────────────────────────────────

export interface UsageRingProps {
  /** Used percent; `undefined` draws an empty ring with a dash. */
  pct?: number;
  severity: Severity;
  size?: number;
  strokeWidth?: number;
}

/** A small progress ring with the rounded percentage inside. */
export const UsageRing: React.FC<UsageRingProps> = ({
  pct,
  severity,
  size = 22,
  strokeWidth = 2.5,
}) => {
  const color = severityColor(severity);
  const paths = useMemo(() => {
    const inset = strokeWidth / 2;
    const rect = Skia.XYWHRect(inset, inset, size - strokeWidth, size - strokeWidth);
    const track = Skia.PathBuilder.Make().addOval(rect).detach();
    const sweep = pct === undefined ? 0 : (clampPct(pct) / 100) * 360;
    const arc = Skia.PathBuilder.Make()
      .addArc(rect, -90, Math.min(Math.max(sweep, 4), 359.9))
      .detach();
    return { track, arc, hasArc: sweep > 0 };
  }, [pct, size, strokeWidth]);
  const box = { width: size, height: size };
  const label = pct === undefined ? '–' : String(Math.round(Math.min(pct, 999)));
  return (
    <View style={box}>
      <Canvas style={box}>
        <Path path={paths.track} color={OkenaColors.borderLight} style="stroke" strokeWidth={strokeWidth} />
        {paths.hasArc ? (
          <Path
            path={paths.arc}
            color={color}
            style="stroke"
            strokeWidth={strokeWidth}
            strokeCap="round"
          />
        ) : null}
      </Canvas>
      <View style={[StyleSheet.absoluteFill, styles.ringCenter]}>
        <Text
          style={[
            styles.ringText,
            { color: pct === undefined ? OkenaColors.textTertiary : color },
            label.length > 2 && styles.ringTextSmall,
          ]}
          allowFontScaling={false}
        >
          {label}
        </Text>
      </View>
    </View>
  );
};

// ── Bar ──────────────────────────────────────────────────────────────────

export interface UsageBarProps {
  usedPercent: number;
  /** Time elapsed (0–100); draws the pace tick and the over-pace band. */
  timePct?: number;
  segments: Segments;
  height?: number;
}

/**
 * The window bar, as the desktop draws it: a fill colored by absolute
 * severity, the part ahead of the time marker in warning / critical, a
 * divider per block, the current block highlighted, and a tick at the time
 * elapsed.
 */
export const UsageBar: React.FC<UsageBarProps> = ({ usedPercent, timePct, segments, height = 8 }) => {
  const used = clampPct(usedPercent);
  const time = timePct === undefined ? undefined : clampPct(timePct);
  const overage =
    time !== undefined && used > time
      ? { start: time, width: used - time, color: used - time > 15 ? OkenaColors.error : OkenaColors.warning }
      : undefined;
  return (
    <View style={[styles.barWrap, { height: height + 6 }]}>
      <View style={[styles.barTrack, { height, borderRadius: height / 2 }]}>
        <View
          style={[styles.barFill, { width: `${used}%`, backgroundColor: severityColor(absSeverity(usedPercent)) }]}
        />
        {overage ? (
          <View
            style={[
              styles.barFill,
              { left: `${overage.start}%`, width: `${overage.width}%`, backgroundColor: overage.color },
            ]}
          />
        ) : null}
        {segments.current ? (
          <View
            style={[
              styles.barCurrent,
              {
                left: `${segments.current[0] * 100}%`,
                width: `${(segments.current[1] - segments.current[0]) * 100}%`,
              },
            ]}
          />
        ) : null}
        {segments.dividers.map((f) => (
          <View
            key={f}
            style={[
              styles.barDivider,
              { left: `${f * 100}%`, backgroundColor: f * 100 > used ? DIVIDER_ON_TRACK : DIVIDER_ON_FILL },
            ]}
          />
        ))}
      </View>
      {time !== undefined ? (
        <View
          style={[styles.barMarker, { left: `${time}%`, height: height + 6 }]}
          accessibilityElementsHidden
          importantForAccessibility="no"
        />
      ) : null}
    </View>
  );
};

const DIVIDER_ON_TRACK = 'rgba(255,255,255,0.10)';
const DIVIDER_ON_FILL = 'rgba(0,0,0,0.35)';

// ── Sparkline ────────────────────────────────────────────────────────────

export interface UsageSparklineProps {
  /** Points on the bar's axes: x 0–1 (time), y 0–1 (used / 100). */
  points: readonly { x: number; y: number }[];
  severity: Severity;
  height?: number;
}

/**
 * History of a window across the bar's time axis, so it lines up with the bar
 * above it. The y axis zooms to the data (at least 0–25 %) to show the shape.
 */
export const UsageSparkline: React.FC<UsageSparklineProps> = ({ points, severity, height = 28 }) => {
  const [width, setWidth] = useState(0);
  const onLayout = (e: LayoutChangeEvent) => setWidth(Math.round(e.nativeEvent.layout.width));
  const color = severityColor(severity);
  const paths = useMemo(() => {
    if (width <= 0 || points.length < 2) return null;
    const yMax = Math.min(Math.max(...points.map((p) => p.y), 0.25) * 1.1, 1);
    const px = (x: number) => Math.min(Math.max(x, 0), 1) * width;
    const py = (y: number) => height - 1 - (Math.min(y, yMax) / yMax) * (height - 2);
    const line = Skia.PathBuilder.Make();
    const area = Skia.PathBuilder.Make();
    points.forEach((p, i) => {
      if (i === 0) {
        line.moveTo(px(p.x), py(p.y));
        area.moveTo(px(p.x), height);
        area.lineTo(px(p.x), py(p.y));
      } else {
        line.lineTo(px(p.x), py(p.y));
        area.lineTo(px(p.x), py(p.y));
      }
    });
    area.lineTo(px(points[points.length - 1]!.x), height);
    area.close();
    return { line: line.detach(), area: area.detach() };
  }, [points, width, height]);
  return (
    <View style={{ height }} onLayout={onLayout} accessibilityElementsHidden importantForAccessibility="no">
      {paths ? (
        <Canvas style={{ width, height }}>
          <Path path={paths.area} color={color} opacity={0.15} />
          <Path path={paths.line} color={color} style="stroke" strokeWidth={1.5} strokeJoin="round" />
        </Canvas>
      ) : (
        <View style={styles.sparkEmpty} />
      )}
    </View>
  );
};

const styles = StyleSheet.create({
  ringCenter: { alignItems: 'center', justifyContent: 'center' },
  ringText: { fontSize: 8.5, fontWeight: '700', letterSpacing: -0.3 },
  ringTextSmall: { fontSize: 7 },
  barWrap: { justifyContent: 'center' },
  barTrack: { overflow: 'hidden', backgroundColor: OkenaColors.surfaceOverlay },
  barFill: { position: 'absolute', top: 0, bottom: 0, left: 0 },
  barCurrent: { position: 'absolute', top: 0, bottom: 0, backgroundColor: 'rgba(232,232,236,0.14)' },
  barDivider: { position: 'absolute', top: 0, bottom: 0, width: 1.5, marginLeft: -0.75 },
  barMarker: {
    position: 'absolute',
    top: 0,
    width: 2,
    marginLeft: -1,
    borderRadius: 1,
    backgroundColor: OkenaColors.textPrimary,
  },
  sparkEmpty: {
    flex: 1,
    borderBottomWidth: StyleSheet.hairlineWidth,
    borderBottomColor: OkenaColors.border,
  },
});
