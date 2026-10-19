/// <reference types="jest" />

import React from 'react';
import { Text, TextInput } from 'react-native';
import TestRenderer, { act } from 'react-test-renderer';
import { ProjectDrawer } from './ProjectDrawer';
import type { ProjectInfo } from '../native/okena';
import { useWorkspaceStore } from '../state/workspaceStore';
import { useConnectionStore } from '../state/connectionStore';
import { useSettingsStore } from '../state/settingsStore';

jest.mock('./Icon', () => ({ Icon: () => null }));
jest.mock('../screens/SettingsSheet', () => ({ SettingsSheet: () => null }));

const native = {
  focusTerminal: jest.fn(async () => {}),
  setProjectColor: jest.fn(async () => {}),
  setFolderColor: jest.fn(async () => {}),
  addProject: jest.fn(async () => {}),
  reorderProjectInFolder: jest.fn(async () => {}),
  closeTerminal: jest.fn(async () => {}),
  createTerminal: jest.fn(async () => {}),
};

function project(index: number): ProjectInfo {
  return {
    id: `p${index}`,
    name: `Project ${index}`,
    path: `/projects/${index}`,
    showInOverview: true,
    terminalIds: [],
    terminalNames: {},
    gitLinesAdded: 0,
    gitLinesRemoved: 0,
    services: [],
    folderColor: 'blue',
    worktreeIds: [],
    terminalAgentStatus: {},
    pinned: false,
  };
}

describe('project drawer', () => {
  let renderer: TestRenderer.ReactTestRenderer;

  beforeEach(() => {
    jest.useFakeTimers();
    const projects = Array.from({ length: 200 }, (_, i) => project(i));
    useWorkspaceStore.setState({
      projects,
      folders: [],
      projectOrder: projects.map((p) => p.id),
      selectedProjectId: null,
      selectedTerminalId: null,
      projectSignals: {},
    });
    useConnectionStore.setState({ connId: null, activeServer: null });
    useSettingsStore.setState({ projectSortMode: 'manual' });
    act(() => {
      renderer = TestRenderer.create(
        <ProjectDrawer open native={native} onClose={() => {}} />,
      );
    });
  });

  afterEach(() => {
    act(() => renderer.unmount());
    jest.useRealTimers();
  });

  function texts(): unknown[] {
    return renderer.root.findAllByType(Text).map((node) => node.props.children);
  }

  it('shows cached projects immediately without mounting the whole workspace', () => {
    const labels = texts().filter(
      (text) => typeof text === 'string' && text.startsWith('Project '),
    );
    expect(labels).toContain('Project 0');
    expect(labels.length).toBeLessThan(30);
    expect(labels).not.toContain('Project 199');
  });

  it('searches projects that were outside the rendered window', () => {
    act(() =>
      renderer.root.findByType(TextInput).props.onChangeText('Project 199'),
    );
    expect(texts()).toContain('Project 199');
    expect(texts()).not.toContain('Project 0');
  });

  it('also virtualizes and searches the activity order', () => {
    act(() => useSettingsStore.setState({ projectSortMode: 'activity' }));
    const labels = texts().filter(
      (text) => typeof text === 'string' && text.startsWith('Project '),
    );
    expect(labels.length).toBeGreaterThan(0);
    expect(labels.length).toBeLessThan(30);
    act(() =>
      renderer.root.findByType(TextInput).props.onChangeText('Project 199'),
    );
    expect(texts()).toContain('Project 199');
  });

  it('reorders a filtered folder member using its original folder position', () => {
    act(() => {
      useConnectionStore.setState({ connId: 'conn-1' });
      useWorkspaceStore.setState({
        projects: [project(0), project(1), project(2)],
        folders: [
          {
            id: 'folder',
            name: 'My folder',
            folderColor: 'blue',
            projectIds: ['p0', 'p1', 'p2'],
          },
        ],
        projectOrder: ['folder'],
      });
      renderer.root.findByType(TextInput).props.onChangeText('Project 2');
    });
    expect(texts()).toContain('Project 2');
    function button(label: string) {
      const found = renderer.root
        .findAll(
          (node) =>
            typeof node.props.onPress === 'function' ||
            typeof node.props.onLongPress === 'function',
        )
        .find((node) =>
          node
            .findAllByType(Text)
            .some((text) => text.props.children === label),
        );
      if (!found) throw new Error(`Button not found: ${label}`);
      return found;
    }
    act(() => button('Project 2').props.onLongPress());
    act(() => button('Move Up').props.onPress());
    expect(native.reorderProjectInFolder).toHaveBeenCalledWith(
      'conn-1',
      'folder',
      'p2',
      1,
    );
  });

  it('keeps folder headers and nested worktrees when flattening the list', () => {
    const parent = { ...project(0), worktreeIds: ['p1'] };
    const child = {
      ...project(1),
      parentProjectId: 'p0',
      gitBranch: 'feature',
    };
    act(() =>
      useWorkspaceStore.setState({
        projects: [parent, child],
        folders: [
          {
            id: 'folder',
            name: 'My folder',
            folderColor: 'blue',
            projectIds: ['p0'],
          },
        ],
        projectOrder: ['folder'],
      }),
    );
    expect(texts()).toEqual(
      expect.arrayContaining(['My folder', 'Project 0', 'feature']),
    );
  });
});
