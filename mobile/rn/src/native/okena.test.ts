/// <reference types="jest" />
// tsconfig pins `types` to react/react-native, so pull jest globals in here.

/**
 * The adapter over the ubrn module turns its `Map`-typed fields and returns
 * into the plain records the app reads.
 */

jest.mock(
  'okena-mobile-ffi',
  () => ({
    getCertFingerprint: () => undefined,
    getProjects: () => [
      {
        id: 'p1',
        name: 'okena',
        path: '/tmp/okena',
        showInOverview: true,
        terminalIds: ['t1', 't2'],
        terminalNames: new Map([['t1', 'claude']]),
        gitLinesAdded: 0,
        gitLinesRemoved: 0,
        services: [],
        folderColor: 'blue',
        terminalAgentStatus: new Map<string, unknown>([
          ['t1', { lifecycle: 'blocked', custom: 'waiting' }],
          ['t2', { lifecycle: 'dreaming', custom: undefined }],
        ]),
      },
    ],
    getClaudeDialogCounts: () =>
      new Map<string, unknown>([
        ['t1', 2],
        ['t2', -1],
      ]),
  }),
  { virtual: true },
);

import { getOkenaNative } from './okena';

describe('getOkenaNative adapters', () => {
  it('converts project map fields, dropping unknown lifecycles', () => {
    const [project] = getOkenaNative().getProjects('conn');
    expect(project!.terminalNames).toEqual({ t1: 'claude' });
    expect(project!.terminalAgentStatus).toEqual({
      t1: { lifecycle: 'blocked', custom: 'waiting' },
    });
  });

  it('converts dialog counts, dropping invalid ones', () => {
    expect(getOkenaNative().getClaudeDialogCounts('conn')).toEqual({ t1: 2 });
  });
});
