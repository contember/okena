/**
 * okena-terminal-view — the native terminal renderer as a React component.
 *
 * `OkenaTerminalView` is the codegen host component. Use a ref with
 * `OkenaTerminalViewCommands.scrollToBottom(ref)` / `.clearSelection(ref)`.
 */

import * as Spec from './OkenaTerminalViewNativeComponent';

// Re-exported under new names: the codegen Babel plugin reserves an export
// named `Commands` for the spec file itself.
export const OkenaTerminalView = Spec.default;
export const OkenaTerminalViewCommands: Spec.NativeCommands = Spec.Commands;

export type {
  NativeProps as OkenaTerminalViewProps,
  NativeCommands as OkenaTerminalViewNativeCommands,
  OkenaTerminalViewType,
  TerminalViewTheme,
  TapEvent,
  GridSizeEvent,
  FontSizeChangeEvent,
  SelectionCopiedEvent,
  ScrollStateChangeEvent,
} from './OkenaTerminalViewNativeComponent';
