/**
 * KeyIcons.tsx — tiny key-cap icons drawn from plain Views.
 *
 * No font glyphs (arrows / ⏎ / ⌨ are missing or emoji-styled in some system
 * fonts) and no Skia (it can't load under jest), so these look the same on
 * iOS, Android and in tests. Triangles use the zero-size border trick.
 */

import React from 'react';
import { StyleSheet, View, type ViewStyle } from 'react-native';

import type { KeyIconName } from './keyInput';

const STROKE = 2;

type Dir = 'up' | 'down' | 'left' | 'right';

/** A solid isosceles triangle pointing `dir`, `size` deep and 2·`half` wide. */
const Triangle: React.FC<{ dir: Dir; color: string; size: number; half: number }> = ({
  dir,
  color,
  size,
  half,
}) => {
  const style: ViewStyle = { width: 0, height: 0, borderStyle: 'solid' };
  if (dir === 'up' || dir === 'down') {
    style.borderLeftWidth = half;
    style.borderRightWidth = half;
    style.borderLeftColor = 'transparent';
    style.borderRightColor = 'transparent';
    if (dir === 'up') {
      style.borderBottomWidth = size;
      style.borderBottomColor = color;
    } else {
      style.borderTopWidth = size;
      style.borderTopColor = color;
    }
  } else {
    style.borderTopWidth = half;
    style.borderBottomWidth = half;
    style.borderTopColor = 'transparent';
    style.borderBottomColor = 'transparent';
    if (dir === 'left') {
      style.borderRightWidth = size;
      style.borderRightColor = color;
    } else {
      style.borderLeftWidth = size;
      style.borderLeftColor = color;
    }
  }
  return <View style={style} />;
};

/** Arrow: triangle head + stem. */
const Arrow: React.FC<{ dir: Dir; color: string }> = ({ dir, color }) => {
  const vertical = dir === 'up' || dir === 'down';
  const head = <Triangle dir={dir} color={color} size={7} half={6} />;
  const stem = (
    <View
      style={[vertical ? styles.stemV : styles.stemH, { backgroundColor: color }]}
    />
  );
  const first = dir === 'up' || dir === 'left';
  return (
    <View style={[styles.center, !vertical && styles.row]}>
      {first ? head : stem}
      {first ? stem : head}
    </View>
  );
};

/** Return: a corner arrow (stem down the right, foot to the left, head left). */
const ReturnIcon: React.FC<{ color: string }> = ({ color }) => (
  <View style={styles.returnBox}>
    <View style={[styles.returnStem, { backgroundColor: color }]} />
    <View style={styles.returnFoot}>
      <Triangle dir="left" color={color} size={6} half={5} />
      <View style={[styles.returnBar, { backgroundColor: color }]} />
    </View>
  </View>
);

/** Keyboard: outlined body with two rows of keys and a space bar. */
const KeyboardIcon: React.FC<{ color: string }> = ({ color }) => {
  const dot = <View style={[styles.kbDot, { backgroundColor: color }]} />;
  return (
    <View style={[styles.kbBody, { borderColor: color }]}>
      <View style={styles.kbRow}>
        {dot}
        {dot}
        {dot}
        {dot}
      </View>
      <View style={styles.kbRow}>
        {dot}
        {dot}
        {dot}
        {dot}
      </View>
      <View style={[styles.kbSpace, { backgroundColor: color }]} />
    </View>
  );
};

const MoreIcon: React.FC<{ color: string }> = ({ color }) => (
  <View style={styles.moreRow}>
    <View style={[styles.moreDot, { backgroundColor: color }]} />
    <View style={[styles.moreDot, { backgroundColor: color }]} />
    <View style={[styles.moreDot, { backgroundColor: color }]} />
  </View>
);

export const KeyIcon: React.FC<{ name: KeyIconName; color: string }> = ({ name, color }) => {
  switch (name) {
    case 'up':
    case 'down':
    case 'left':
    case 'right':
      return <Arrow dir={name} color={color} />;
    case 'return':
      return <ReturnIcon color={color} />;
    case 'keyboard':
      return <KeyboardIcon color={color} />;
    case 'more':
      return <MoreIcon color={color} />;
  }
};

/** Gear-like settings glyph: a ring with eight teeth (four crossed bars). */
export const SettingsIcon: React.FC<{ color: string; background: string; size?: number }> = ({
  color,
  background,
  size = 20,
}) => {
  const tooth = size * 0.22;
  const ring = size * 0.66;
  const hole = size * 0.28;
  return (
    <View style={[styles.center, { width: size, height: size }]}>
      {[0, 45, 90, 135].map((deg) => (
        <View
          key={deg}
          style={[
            styles.gearBar,
            {
              width: tooth,
              height: size,
              borderRadius: tooth / 3,
              backgroundColor: color,
              transform: [{ rotate: `${deg}deg` }],
            },
          ]}
        />
      ))}
      <View
        style={[
          styles.center,
          { width: ring, height: ring, borderRadius: ring / 2, backgroundColor: color },
        ]}
      >
        <View
          style={{
            width: hole,
            height: hole,
            borderRadius: hole / 2,
            backgroundColor: background,
          }}
        />
      </View>
    </View>
  );
};

const styles = StyleSheet.create({
  center: { alignItems: 'center', justifyContent: 'center' },
  row: { flexDirection: 'row' },
  stemV: { width: STROKE, height: 8 },
  stemH: { width: 8, height: STROKE },
  returnBox: { width: 18, height: 14 },
  returnStem: {
    position: 'absolute',
    right: 1,
    top: 0,
    width: STROKE,
    height: 10,
  },
  returnFoot: {
    position: 'absolute',
    left: 0,
    right: 1,
    top: 4,
    flexDirection: 'row',
    alignItems: 'center',
  },
  returnBar: { flex: 1, height: STROKE, marginTop: 0 },
  kbBody: {
    width: 22,
    height: 15,
    borderWidth: 1.5,
    borderRadius: 3,
    paddingHorizontal: 2,
    paddingTop: 2,
    alignItems: 'center',
  },
  kbRow: { flexDirection: 'row', justifyContent: 'space-between', alignSelf: 'stretch', marginBottom: 1.5 },
  kbDot: { width: 2.5, height: 2, borderRadius: 0.5 },
  kbSpace: { width: 9, height: 1.5, borderRadius: 0.75, marginTop: 0.5 },
  moreRow: { flexDirection: 'row', alignItems: 'center' },
  moreDot: { width: 4, height: 4, borderRadius: 2, marginHorizontal: 2 },
  gearBar: { position: 'absolute' },
});
