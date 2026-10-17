/// <reference types="jest" />

import {
  INITIAL_SOFT_INPUT,
  SENTINELS,
  applyChange,
  diffInput,
  requestReset,
  shouldResetAfter,
  type SoftInputState,
} from './softInput';

const S0 = SENTINELS[0];
const S1 = SENTINELS[1];

/** Feed native field contents through the state machine, collecting input. */
function type(state: SoftInputState, ...texts: string[]) {
  const sent: string[] = [];
  for (const t of texts) {
    const r = applyChange(state, t);
    state = r.state;
    sent.push('⌫'.repeat(r.delta.backspaces) + r.delta.text);
  }
  return { state, sent };
}

describe('diffInput', () => {
  it('sends appended text', () => {
    expect(diffInput(`${S0}ab`, `${S0}abc`)).toEqual({ backspaces: 0, text: 'c' });
  });

  it('sends backspaces for deleted text', () => {
    expect(diffInput(`${S0}abc`, `${S0}a`)).toEqual({ backspaces: 2, text: '' });
  });

  it('replaces a word picked from QuickType', () => {
    expect(diffInput(`${S0}pris`, `${S0}příště `)).toEqual({ backspaces: 3, text: 'říště ' });
  });

  it('turns a dead-key diacritic into backspace + composed letter', () => {
    expect(diffInput(`${S0}ˇ`, `${S0}č`)).toEqual({ backspaces: 1, text: 'č' });
  });

  it('counts an emoji as one character', () => {
    expect(diffInput(`${S0}a😀`, `${S0}a`)).toEqual({ backspaces: 1, text: '' });
  });

  it('retypes the tail after an edit in the middle', () => {
    expect(diffInput(`${S0}hello`, `${S0}hexllo`)).toEqual({ backspaces: 3, text: 'xllo' });
  });
});

describe('soft input state', () => {
  it('does not resend earlier messages after a reset', () => {
    let { state } = type(INITIAL_SOFT_INPUT, `${S0}e`, `${S0}echo one`);
    state = requestReset(state)!;
    expect(state.text).toBe(S1);
    // The reset landed: native now holds the other sentinel.
    const r = type(state, `${S1}x`);
    expect(r.sent).toEqual(['x']);
  });

  it('diffs against the old text when native rejected the reset', () => {
    let { state } = type(INITIAL_SOFT_INPUT, `${S0}abc`);
    state = requestReset(state)!;
    // Typing raced the reset, so native kept the old text.
    const r = type(state, `${S0}abcd`);
    expect(r.sent).toEqual(['d']);
    expect(r.state.pendingFrom).toBeNull();
  });

  it('skips a reset when nothing was typed', () => {
    expect(requestReset(INITIAL_SOFT_INPUT)).toBeNull();
  });

  it('keeps the unconfirmed text across repeated resets', () => {
    let { state } = type(INITIAL_SOFT_INPUT, `${S0}abc`);
    state = requestReset(state)!;
    expect(requestReset(state)).toBeNull();
    expect(type(state, `${S0}abcd`).sent).toEqual(['d']);
  });

  it('resets after a newline and when the buffer runs low or long', () => {
    const nl = applyChange(INITIAL_SOFT_INPUT, `${S0}ls\n`);
    expect(shouldResetAfter(nl.state, nl.delta)).toBe(true);
    const low = applyChange(INITIAL_SOFT_INPUT, '  ');
    expect(shouldResetAfter(low.state, low.delta)).toBe(true);
    const long = applyChange(INITIAL_SOFT_INPUT, S0 + 'a'.repeat(200));
    expect(shouldResetAfter(long.state, long.delta)).toBe(true);
    const plain = applyChange(INITIAL_SOFT_INPUT, `${S0}a`);
    expect(shouldResetAfter(plain.state, plain.delta)).toBe(false);
  });
});
