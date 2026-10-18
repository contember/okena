/**
 * Codegen spec for the native terminal renderer (Fabric, RN new architecture).
 *
 * This file is the contract between JS and the Swift / Kotlin views. Codegen
 * reads it at build time (`codegenConfig` in package.json) and generates the
 * props struct, event emitter and command handlers for both platforms. Keep
 * the native sides in sync when changing it:
 *   - iOS:     ios/OkenaTerminalViewComponentView.mm (props, events, commands)
 *   - Android: android/src/main/java/com/okena/terminalview/OkenaTerminalViewManager.kt
 */

import type * as React from 'react';
import { codegenNativeCommands, codegenNativeComponent } from 'react-native';
import type { CodegenTypes, HostComponent, ViewProps } from 'react-native';

/**
 * Terminal colors as packed unsigned ARGB (`0xAARRGGBB`), the same encoding
 * the Rust core uses for resolved cell colors. Doubles, not Int32: opaque
 * colors exceed the signed 32-bit range.
 */
export type TerminalViewTheme = Readonly<{
  background: CodegenTypes.Double;
  foreground: CodegenTypes.Double;
  cursor: CodegenTypes.Double;
  /** Selection highlight overlay drawn over selected cells. */
  selection: CodegenTypes.Double;
}>;

export type TapEvent = Readonly<{
  col: CodegenTypes.Int32;
  row: CodegenTypes.Int32;
}>;

export type GridSizeEvent = Readonly<{
  cols: CodegenTypes.Int32;
  rows: CodegenTypes.Int32;
  cellWidth: CodegenTypes.Double;
  cellHeight: CodegenTypes.Double;
}>;

export type FontSizeChangeEvent = Readonly<{
  size: CodegenTypes.Double;
}>;

export type SelectionCopiedEvent = Readonly<{
  text: string;
}>;

export type ScrollStateChangeEvent = Readonly<{
  atBottom: boolean;
}>;

export type OutputPausedChangeEvent = Readonly<{
  paused: boolean;
}>;

export interface NativeProps extends ViewProps {
  /** Connection id from the Rust `ConnectionManager`. */
  connId: string;
  terminalId: string;
  /** Font size in pt (dp on Android). */
  fontSize: CodegenTypes.Double;
  theme: TerminalViewTheme;
  /** Whether this pane is the keyboard / toolbar target (cursor style). */
  focused?: CodegenTypes.WithDefault<boolean, false>;

  /** A single tap at a cell. JS focuses the hidden TextInput. */
  onTap?: CodegenTypes.DirectEventHandler<TapEvent>;
  /** Grid geometry from layout and font metrics. The view resizes the PTY itself. */
  onGridSize?: CodegenTypes.DirectEventHandler<GridSizeEvent>;
  /** Pinch ended at a new font size; JS persists it. */
  onFontSizeChange?: CodegenTypes.DirectEventHandler<FontSizeChangeEvent>;
  /** A selection was copied to the system clipboard by the view. */
  onSelectionCopied?: CodegenTypes.DirectEventHandler<SelectionCopiedEvent>;
  /** The viewport moved to or away from the live bottom. */
  onScrollStateChange?: CodegenTypes.DirectEventHandler<ScrollStateChangeEvent>;
  /**
   * Live output was frozen (`paused`) while a selection gesture runs, or
   * resumed and caught up. The view freezes and resumes on its own.
   */
  onOutputPausedChange?: CodegenTypes.DirectEventHandler<OutputPausedChangeEvent>;
}

export type OkenaTerminalViewType = HostComponent<NativeProps>;

export interface NativeCommands {
  scrollToBottom: (viewRef: React.ElementRef<OkenaTerminalViewType>) => void;
  clearSelection: (viewRef: React.ElementRef<OkenaTerminalViewType>) => void;
  /**
   * Open the native "Select text" mode: a full-screen, frozen plain-text copy
   * of the recent scrollback with the platform's own selection UI.
   */
  openSelectText: (viewRef: React.ElementRef<OkenaTerminalViewType>) => void;
}

export const Commands: NativeCommands = codegenNativeCommands<NativeCommands>({
  supportedCommands: ['scrollToBottom', 'clearSelection', 'openSelectText'],
});

export default codegenNativeComponent<NativeProps>(
  'OkenaTerminalView',
) as OkenaTerminalViewType;
