/**
 * Icon.tsx — cached raster icon displayed by the native Image view.
 *
 * Replaces Unicode "icon" characters in `<Text>`, which render as "?" boxes
 * when the platform fonts lack them. Path data lives in `iconPaths.ts`.
 */

import React, { useMemo } from 'react';
import {
  Image,
  PixelRatio,
  View,
  type ImageSourcePropType,
  type StyleProp,
  type ViewStyle,
} from 'react-native';
import {
  PaintStyle,
  Skia,
  StrokeCap,
  StrokeJoin,
  type SkPath,
} from '@shopify/react-native-skia';

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

const imageCache = new Map<string, ImageSourcePropType>();

function iconImage(
  name: IconName,
  size: number,
  strokeWidth: number,
): ImageSourcePropType | undefined {
  const pixels = PixelRatio.getPixelSizeForLayoutSize(size);
  const key = `${name}:${pixels}:${strokeWidth}`;
  const cached = imageCache.get(key);
  if (cached) return cached;
  const path = iconPath(name);
  if (!path) return undefined;
  const surface = Skia.Surface.Make(pixels, pixels);
  if (!surface) return undefined;
  const paint = Skia.Paint();
  paint.setAntiAlias(true);
  paint.setColor(Skia.Color('white'));
  const def: IconDef = ICONS[name];
  paint.setStyle(def.fill ? PaintStyle.Fill : PaintStyle.Stroke);
  paint.setStrokeWidth(strokeWidth);
  paint.setStrokeCap(StrokeCap.Round);
  paint.setStrokeJoin(StrokeJoin.Round);
  const canvas = surface.getCanvas();
  canvas.clear(Skia.Color('transparent'));
  canvas.scale(pixels / 24, pixels / 24);
  canvas.drawPath(path, paint);
  const snapshot = surface.makeImageSnapshot();
  const source = { uri: `data:image/png;base64,${snapshot.encodeToBase64()}` };
  snapshot.dispose();
  paint.dispose();
  surface.dispose();
  imageCache.set(key, source);
  return source;
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

export const Icon: React.FC<IconProps> = ({
  name,
  color,
  size = 18,
  strokeWidth = 2,
  style,
}) => {
  const source = iconImage(name, size, strokeWidth);
  const box = useMemo(() => ({ width: size, height: size }), [size]);
  return (
    <View style={[box, style]} pointerEvents="none">
      {source ? (
        <Image
          source={source}
          style={box}
          tintColor={color}
          accessible={false}
          fadeDuration={0}
        />
      ) : null}
    </View>
  );
};

export default Icon;
