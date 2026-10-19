/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

import { navigate, nextScreen, openClaude, useNavStore } from './navStore';

describe('claude navigation', () => {
  it('stays on the Claude chat while connected, leaves it on disconnect', () => {
    expect(nextScreen('claude', 'workspace')).toBe('claude');
    expect(nextScreen('claude', 'pairing')).toBe('pairing');
    expect(nextScreen('claude', 'serverList')).toBe('serverList');
    expect(nextScreen('workspace', 'workspace')).toBe('workspace');
  });

  it('carries the terminal id only while on the Claude chat', () => {
    openClaude('t1');
    expect(useNavStore.getState()).toMatchObject({
      screen: 'claude',
      claudeTerminalId: 't1',
    });
    navigate('workspace');
    expect(useNavStore.getState()).toMatchObject({
      screen: 'workspace',
      claudeTerminalId: null,
    });
  });
});
