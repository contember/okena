/// <reference types="jest" />

import React from 'react';
import { Text } from 'react-native';
import TestRenderer, { act } from 'react-test-renderer';

import { ClaudeDialogCard } from './ClaudeDialogCard';
import type { ApiClaudeDialog } from '../native/claude';
import type { ClaudeDialogAnswer } from '../state';

const PERMISSION: ApiClaudeDialog = {
  id: 'd1',
  tool_name: 'Bash',
  tool_input: { command: 'ls' },
  permission_suggestions: [{ type: 'addRules' }],
  created_at_ms: 1,
};

const PLAN: ApiClaudeDialog = {
  ...PERMISSION,
  tool_name: 'ExitPlanMode',
  tool_input: { plan: 'Do it' },
};

const QUESTION: ApiClaudeDialog = {
  ...PERMISSION,
  tool_name: 'AskUserQuestion',
  tool_input: {
    questions: [{ question: 'Which?', options: [{ label: 'A' }] }],
  },
};

describe('claude dialog card', () => {
  let renderer: TestRenderer.ReactTestRenderer;
  const onDecide = jest.fn();

  function render(
    dialog: ApiClaudeDialog,
    answer: ClaudeDialogAnswer | undefined,
  ) {
    act(() => {
      renderer = TestRenderer.create(
        <ClaudeDialogCard
          dialog={dialog}
          answer={answer}
          onDecide={onDecide}
          onRevealStep={() => {}}
        />,
      );
    });
  }

  function rerender(dialog: ApiClaudeDialog, answer: ClaudeDialogAnswer) {
    act(() =>
      renderer.update(
        <ClaudeDialogCard
          dialog={dialog}
          answer={answer}
          onDecide={onDecide}
          onRevealStep={() => {}}
        />,
      ),
    );
  }

  function pressables() {
    return renderer.root.findAll(
      (node) => typeof node.props.onPress === 'function',
    );
  }

  /** `{label → disabled}` of the card's buttons. */
  function buttons(): Record<string, boolean> {
    const out: Record<string, boolean> = {};
    for (const pressable of pressables()) {
      if (pressable.props.accessibilityRole !== 'button') continue;
      const label = pressable.findByType(Text).props.children;
      out[String(label)] = pressable.props.disabled === true;
    }
    return out;
  }

  function press(label: string) {
    const target = pressables().find((pressable) =>
      pressable
        .findAllByType(Text)
        .some((text) => text.props.children === label),
    );
    if (target === undefined) throw new Error(`No button ${label}`);
    act(() => target.props.onPress());
  }

  function texts(): unknown[] {
    return renderer.root.findAllByType(Text).map((node) => node.props.children);
  }

  afterEach(() => {
    act(() => renderer.unmount());
    onDecide.mockClear();
  });

  it('disables every permission action while an answer is sending', () => {
    render(PERMISSION, { status: 'sending' });
    expect(buttons()).toEqual({ Deny: true, Always: true, Allow: true });
    expect(texts()).toContain('Sending answer…');
  });

  it('disables the deny message step while an answer is sending', () => {
    render(PERMISSION, undefined);
    press('Deny');
    rerender(PERMISSION, { status: 'sending' });
    expect(buttons()).toEqual({ Cancel: true, Deny: true });
  });

  it('enables the actions again with the error of a failed answer', () => {
    render(PERMISSION, { status: 'failed', error: 'refused' });
    expect(buttons()).toEqual({ Deny: false, Always: false, Allow: false });
    expect(texts()).toContain('refused');
    press('Allow');
    expect(onDecide).toHaveBeenCalledWith({ decision: 'allow' });
  });

  it('disables plan approval while sending', () => {
    render(PLAN, { status: 'sending' });
    expect(buttons()).toEqual({ 'Keep planning': true, Approve: true });
  });

  it('disables question submit while sending', () => {
    render(QUESTION, undefined);
    press('A');
    expect(buttons()).toEqual({ Submit: false });
    rerender(QUESTION, { status: 'sending' });
    expect(buttons()).toEqual({ Submit: true });
  });

  it('offers only Deny for questions it cannot read', () => {
    render({ ...QUESTION, tool_input: { questions: 'Which?' } }, undefined);
    expect(buttons()).toEqual({ Deny: false });
    press('Deny');
    press('Deny');
    expect(onDecide).toHaveBeenCalledWith({ decision: 'deny' });
  });
});
