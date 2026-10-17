/**
 * softInput.ts — turns edits of the hidden soft-keyboard TextInput into
 * terminal input.
 *
 * The keyboard edits a real text field, not a key stream: autocomplete,
 * QuickType, dictation, dead keys for diacritics (ˇ + c → č) and caret moves
 * replace text anywhere in it. So each change is diffed against the previous
 * text by common prefix: whatever was removed after the prefix becomes
 * Backspaces, whatever was added becomes typed text. The terminal line ends up
 * matching what the field shows.
 *
 * The field holds a sentinel so backspace always has something to delete
 * (Android's soft keyboard doesn't report backspace on an empty field). It is
 * reset back to a sentinel from time to time, through the controlled `value`
 * prop. Native rejects that reset when typing raced it, so the reset target
 * alternates between two sentinels that start with different characters:
 * the next change tells whether the reset landed.
 */

/** The two sentinels; they must differ in their first character. */
export const SENTINELS = [' '.repeat(8), ' '.repeat(8)] as const;

/** What the hidden field holds, as far as JS knows. */
export interface SoftInputState {
  /** The field's text after the last change (or the requested reset). */
  text: string;
  /** Text before a requested reset that native hasn't confirmed yet. */
  pendingFrom: string | null;
}

/** Terminal input for one change of the field. */
export interface InputDelta {
  /** Characters to erase (one Backspace each). */
  backspaces: number;
  /** Text to type after erasing. */
  text: string;
}

export const INITIAL_SOFT_INPUT: SoftInputState = { text: SENTINELS[0], pendingFrom: null };

/** Diff two field contents by common prefix, counting code points. */
export function diffInput(prev: string, next: string): InputDelta {
  const a = Array.from(prev);
  const b = Array.from(next);
  let p = 0;
  while (p < a.length && p < b.length && a[p] === b[p]) p++;
  return { backspaces: a.length - p, text: b.slice(p).join('') };
}

/** True when the field holds only a sentinel (nothing typed since a reset). */
export function isSentinel(text: string): boolean {
  return text === SENTINELS[0] || text === SENTINELS[1];
}

/** Process a native change: the terminal input to send and the next state. */
export function applyChange(
  state: SoftInputState,
  newText: string,
): { delta: InputDelta; state: SoftInputState } {
  let base = state.text;
  if (state.pendingFrom !== null && newText[0] !== state.text[0]) {
    // The reset didn't land: native still has the old text.
    base = state.pendingFrom;
  }
  return { delta: diffInput(base, newText), state: { text: newText, pendingFrom: null } };
}

/**
 * Request a reset to a fresh sentinel. Returns null when there is nothing to
 * reset (the field holds only a sentinel already).
 */
export function requestReset(state: SoftInputState): SoftInputState | null {
  if (isSentinel(state.text)) return null;
  const sentinel = state.text[0] === SENTINELS[0][0] ? SENTINELS[1] : SENTINELS[0];
  return { text: sentinel, pendingFrom: state.pendingFrom ?? state.text };
}

/** Whether to reset after a change that produced `state`. */
export function shouldResetAfter(state: SoftInputState, delta: InputDelta): boolean {
  // A newline submits the line; later edits must not reach back into it.
  if (delta.text.includes('\n')) return true;
  // Backspace ate into the sentinel, or the field grew long.
  return state.text.length < 3 || state.text.length > 200;
}
