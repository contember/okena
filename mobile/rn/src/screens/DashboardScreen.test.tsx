/// <reference types="jest" />

import React from 'react';
import { Text } from 'react-native';
import TestRenderer, { act } from 'react-test-renderer';

import { DashboardScreen } from './DashboardScreen';
import type { ApiClaudeDialog } from '../native/claude';
import type { ProjectInfo } from '../native/okena';
import { useClaudeDialogsStore } from '../state/claudeDialogsStore';
import { useConnectionStore } from '../state/connectionStore';
import { useWorkspaceStore } from '../state/workspaceStore';

jest.mock('../components/Icon', () => ({ Icon: () => null }));
jest.mock('./UsageSheet', () => ({ ConnectedUsageSheet: () => null }));
jest.mock('../components/ClaudeDialogSheet', () => ({
  ClaudeDialogSheet: (props: { title: string }) => {
    const { Text: MockText } = jest.requireActual('react-native');
    return <MockText>{`sheet ${props.title}`}</MockText>;
  },
}));

function project(id: string, extra: Partial<ProjectInfo> = {}): ProjectInfo {
  return {
    id,
    name: id,
    path: `/projects/${id}`,
    showInOverview: true,
    terminalIds: [`${id}-t`],
    terminalNames: { [`${id}-t`]: 'claude' },
    gitLinesAdded: 0,
    gitLinesRemoved: 0,
    services: [],
    folderColor: 'blue',
    worktreeIds: [],
    terminalAgentStatus: {},
    pinned: false,
    ...extra,
  };
}

const QUESTION: ApiClaudeDialog = {
  id: 'd1',
  tool_name: 'AskUserQuestion',
  tool_input: { questions: [{ question: 'Which layout?', options: [{ label: 'A' }] }] },
  permission_suggestions: [],
  created_at_ms: 1,
};

describe('dashboard', () => {
  let renderer: TestRenderer.ReactTestRenderer;
  const showWorkspace = jest.fn();

  beforeEach(() => {
    showWorkspace.mockClear();
    useConnectionStore.setState({ connId: 'conn-1', activeServer: null });
    useWorkspaceStore.setState({
      projects: [project('okena'), project('kvalt', { pinned: true, gitBranch: 'main' })],
      folders: [],
      projectOrder: ['okena', 'kvalt'],
      projectSignals: {},
      attention: [
        { projectId: 'okena', terminalId: 'okena-t', kind: 'dialog', dialogs: 1 },
        { projectId: 'kvalt', terminalId: 'kvalt-t', kind: 'bell', dialogs: 0 },
      ],
      showWorkspace,
    });
    // The previews are given; polling is the store's own test.
    useClaudeDialogsStore.setState({
      dialogs: { 'okena-t': [QUESTION] },
      open: () => {},
      close: () => {},
    });
    act(() => {
      renderer = TestRenderer.create(<DashboardScreen />);
    });
  });

  afterEach(() => {
    act(() => renderer.unmount());
  });

  const texts = () => renderer.root.findAllByType(Text).map((t) => t.props.children);
  const row = (label: string) =>
    renderer.root.find(
      (node) =>
        typeof node.props.onPress === 'function' &&
        typeof node.props.accessibilityLabel === 'string' &&
        node.props.accessibilityLabel.startsWith(label),
    );

  it('previews the Claude question and lists pinned projects', () => {
    expect(texts()).toContain('Which layout?');
    expect(texts()).toContain('NEEDS YOU · 2');
    expect(texts()).toContain('PINNED PROJECTS · 1');
    expect(texts()).toContain('main');
  });

  it('opens a Claude dialog in the sheet and anything else in its terminal', () => {
    act(() => row('okena · claude').props.onPress());
    expect(texts()).toContain('sheet okena · claude');
    expect(showWorkspace).not.toHaveBeenCalled();

    act(() => row('kvalt · claude').props.onPress());
    expect(showWorkspace).toHaveBeenCalledWith('kvalt', 'kvalt-t');
  });
});
