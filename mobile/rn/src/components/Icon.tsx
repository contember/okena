/**
 * Icon.tsx — small vector icon drawn with react-native-skia.
 *
 * Replaces Unicode "icon" characters in `<Text>`, which render as "?" boxes
 * when the platform fonts lack them. Path data lives in `iconPaths.ts`.
 */

import React, { useMemo } from 'react';
import { View, type StyleProp, type ViewStyle } from 'react-native';
import { Canvas, Group, Path, Skia, type SkPath } from '@shopify/react-native-skia';

import { ICONS, type IconDef, type IconName } from './iconPaths';

export type { IconName } from './iconPaths';

const pathCache = new Map<IconName, SkPath | null>();

function iconPath(name: IconName): SkPath | null {
  let path = pathCache.get(name);
  if (path === undefined) {
    path = Skia.Path.MakeFromSVGString(ICONS[name].d);
    pathCache.set(name, path);
  }
  return path;
}

export interface IconProps {
  name: IconName;
  color: string;
  /** Edge length in logical px (default 18). */
  size?: number;
  /** Stroke width in the 24-unit icon box (default 2). */
  strokeWidth?: number;
  style?: StyleProp<ViewStyle>;
}

export const Icon: React.FC<IconProps> = ({ name, color, size = 18, strokeWidth = 2, style }) => {
  const path = iconPath(name);
  const def: IconDef = ICONS[name];
  const fill = def.fill === true;
  const transform = useMemo(() => [{ scale: size / 24 }], [size]);
  const box = useMemo(() => ({ width: size, height: size }), [size]);
  return (
    <View style={[box, style]} pointerEvents="none">
      {path ? (
        <Canvas style={box}>
          <Group transform={transform}>
            <Path
              path={path}
              color={color}
              style={fill ? 'fill' : 'stroke'}
              strokeWidth={strokeWidth}
              strokeCap="round"
              strokeJoin="round"
            />
          </Group>
        </Canvas>
      ) : null}
    </View>
  );
};

export default Icon;
