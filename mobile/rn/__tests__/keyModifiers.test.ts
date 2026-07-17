jest.mock('@react-native-clipboard/clipboard', () =>
  require('@react-native-clipboard/clipboard/jest/clipboard-mock'),
);

import {
  KeyModifiers,
  applyModifiersToText,
} from '../src/components/KeyToolbar';

describe('KeyModifiers', () => {
  it('uses tap as a one-shot modifier', () => {
    const modifiers = new KeyModifiers();

    modifiers.toggleCtrl();
    expect(modifiers.getSnapshot().ctrl).toBe('active');

    modifiers.reset();
    expect(modifiers.getSnapshot().ctrl).toBe('inactive');
  });

  it('keeps a long-pressed modifier locked across reset', () => {
    const modifiers = new KeyModifiers();

    modifiers.lockOption();
    modifiers.reset();
    expect(modifiers.getSnapshot().option).toBe('locked');

    modifiers.lockOption();
    expect(modifiers.getSnapshot().option).toBe('inactive');
  });

  it('encodes control and meta input', () => {
    const control = new KeyModifiers();
    control.toggleCtrl();
    expect(applyModifiersToText(control, 'c')).toBe('\u0003');

    const meta = new KeyModifiers();
    meta.toggleCmd();
    expect(applyModifiersToText(meta, 'x')).toBe('\u001bx');
  });
});
