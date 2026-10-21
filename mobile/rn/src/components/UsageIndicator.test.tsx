/// <reference types="jest" />

/**
 * Usage UI: the app-bar rings (hidden without data, labelled for screen
 * readers) and the usage sheet (server header, display rules, refresh on open).
 */

import React from 'react';
import { Text } from 'react-native';
import TestRenderer, { act } from 'react-test-renderer';

import type { PoolInfo, UsageInfo } from '../native/okena';
import { UsageIndicator } from './UsageIndicator';
import { UsageSheet } from '../screens/UsageSheet';

jest.mock('@shopify/react-native-skia', () => {
  const R = require('react');
  const builder: Record<string, unknown> = {};
  for (const m of ['addOval', 'addArc', 'moveTo', 'lineTo', 'close']) builder[m] = () => builder;
  builder.detach = () => ({});
  return {
    Canvas: ({ children }: { children?: unknown }) => R.createElement('Canvas', null, children),
    Path: () => R.createElement('SkPath'),
    Skia: { XYWHRect: () => ({}), PathBuilder: { Make: () => builder } },
  };
});
jest.mock('react-native-safe-area-context', () => ({
  useSafeAreaInsets: () => ({ top: 0, bottom: 0, left: 0, right: 0 }),
}));
jest.mock('../native/okena', () => ({ getOkenaNative: () => ({}) }));

function texts(renderer: TestRenderer.ReactTestRenderer): string[] {
  return renderer.root
    .findAllByType(Text)
    .map((t) => {
      const c = t.props.children;
      return Array.isArray(c) ? c.join('') : String(c ?? '');
    });
}

function samplePool(now: number, leftSeconds: number): PoolInfo {
  const amount = (workSeconds: number, accountWeekPercent: number) => ({ workSeconds, accountWeekPercent });
  return {
    today: {
      left: amount(leftSeconds, 2),
      used: amount(3_720, 4),
      safePerDay: amount(7_200, 7),
      offDay: amount(0, 0),
    },
    nonstop: { kind: 'blocked', until: now + 600 },
    pace: { sessionFillSeconds: 7_200, sessionWeekPercent: 10, fromHistory: true },
    accounts: [
      {
        id: 'a1',
        label: 'Work',
        tier: 'Max 5x',
        blocked: '7d limit',
        usedTodayPercent: 2,
        windows: [
          {
            id: 'five_hour',
            label: 'Session',
            kind: 'five_hour',
            usedPercent: 30,
            windowSeconds: 18_000,
            resetAt: now + 9_000,
            active: true,
            history: [],
          },
          {
            id: 'seven_day',
            label: 'Weekly',
            kind: 'weekly',
            usedPercent: 100,
            windowSeconds: 604_800,
            resetAt: now + 86_400,
            active: true,
            history: [],
          },
        ],
      },
    ],
  };
}

describe('UsageIndicator', () => {
  const now = 1_781_773_200;

  beforeEach(() => {
    jest.useFakeTimers();
    jest.setSystemTime(now * 1000);
  });

  afterEach(() => {
    jest.useRealTimers();
  });

  const fiveHour = (usedPercent: number, resetAt: number) => ({
    id: 'five_hour',
    label: 'Session',
    kind: 'five_hour',
    usedPercent,
    windowSeconds: 18_000,
    resetAt,
    active: true,
    history: [],
  });

  function indicatorUsage(usedPercent: number, resetAt: number): UsageInfo {
    return {
      providers: [
        {
          provider: 'claude',
          status: 'ok',
          fetching: false,
          consecutiveFailures: 0,
          windows: [fiveHour(usedPercent, resetAt)],
        },
        { provider: 'codex', status: 'pending', fetching: true, consecutiveFailures: 0, windows: [] },
      ],
      workingDays: [0, 1, 2, 3, 4, 5, 6],
      generatedAt: now,
    };
  }

  function severity(renderer: TestRenderer.ReactTestRenderer): string {
    return renderer.root.findAll((n) => n.props.severity !== undefined)[0]!.props.severity;
  }

  it('renders nothing without usage', () => {
    let renderer!: TestRenderer.ReactTestRenderer;
    act(() => {
      renderer = TestRenderer.create(<UsageIndicator usage={null} onPress={() => {}} />);
    });
    expect(renderer.toJSON()).toBeNull();
  });

  it('shows one ring per provider with an accessible summary', () => {
    const onPress = jest.fn();
    let renderer!: TestRenderer.ReactTestRenderer;
    act(() => {
      renderer = TestRenderer.create(
        <UsageIndicator usage={indicatorUsage(20.4, now + 9_000)} onPress={onPress} />,
      );
    });
    expect(texts(renderer)).toEqual(['20', 'C', '–', 'X']);
    const button = renderer.root.find(
      (n) => n.props.testID === 'usage-indicator' && typeof n.props.onPress === 'function',
    );
    expect(button.props.accessibilityLabel).toBe(
      'Agent usage: Claude 20% of 5h limit; Codex no data yet',
    );
    act(() => button.props.onPress());
    expect(onPress).toHaveBeenCalledTimes(1);
    act(() => renderer.unmount());
  });

  it("shows the pool's Today as text instead of Claude's ring", () => {
    const pooled = indicatorUsage(20, now + 9_000);
    pooled.providers[0] = { ...pooled.providers[0]!, pool: samplePool(now, -45 * 60) };
    let renderer!: TestRenderer.ReactTestRenderer;
    act(() => {
      renderer = TestRenderer.create(<UsageIndicator usage={pooled} onPress={() => {}} />);
    });
    expect(texts(renderer)).toEqual(['45m over', 'C', '–', 'X']);
    act(() => renderer.unmount());
  });

  it('re-derives the pace color as the clock moves', () => {
    // 50 % used, 1h of the 5h window elapsed: far ahead of pace.
    let renderer!: TestRenderer.ReactTestRenderer;
    act(() => {
      renderer = TestRenderer.create(
        <UsageIndicator usage={indicatorUsage(50, now + 14_400)} onPress={() => {}} />,
      );
    });
    expect(severity(renderer)).toBe('critical');
    // Two hours later 3h of 5h (60 %) have elapsed: back within pace.
    act(() => jest.advanceTimersByTime(2 * 3_600_000));
    expect(severity(renderer)).toBe('normal');
    act(() => renderer.unmount());
  });
});

describe('UsageSheet', () => {
  const now = 1_781_773_200;
  const mounted: TestRenderer.ReactTestRenderer[] = [];

  beforeEach(() => {
    jest.useFakeTimers();
    jest.setSystemTime(now * 1000);
  });

  afterEach(() => {
    act(() => mounted.splice(0).forEach((r) => r.unmount()));
    jest.useRealTimers();
  });

  const usage: UsageInfo = {
    providers: [
      {
        provider: 'claude',
        status: 'ok',
        plan: 'max',
        fetchedAt: now - 120,
        fetching: false,
        consecutiveFailures: 0,
        windows: [
          {
            id: 'five_hour',
            label: 'Session',
            kind: 'five_hour',
            usedPercent: 42,
            windowSeconds: 18_000,
            resetAt: now + 9_000,
            active: true,
            history: [
              { recordedAt: now - 7_000, usedPercent: 10 },
              { recordedAt: now - 60, usedPercent: 42 },
            ],
          },
          {
            id: 'seven_day_fable',
            label: 'Fable',
            kind: 'weekly_scoped',
            scopedModel: 'Fable',
            usedPercent: 0.1,
            windowSeconds: 604_800,
            active: false,
            history: [],
          },
        ],
        extraUsage: { isEnabled: true, monthlyLimitCents: 5_000, usedCreditsCents: 125, utilization: 2.5 },
      },
      { provider: 'codex', status: 'disabled', fetching: false, consecutiveFailures: 0, windows: [] },
    ],
    workingDays: [0, 1, 2, 3, 4, 5, 6],
    generatedAt: now,
  };

  function render(props: Partial<React.ComponentProps<typeof UsageSheet>> = {}) {
    const onRefresh = jest.fn();
    let renderer!: TestRenderer.ReactTestRenderer;
    act(() => {
      renderer = TestRenderer.create(
        <UsageSheet
          visible
          onClose={() => {}}
          serverName="Studio Mac"
          usage={usage}
          refreshing={false}
          onRefresh={onRefresh}
          {...props}
        />,
      );
    });
    mounted.push(renderer);
    return { renderer, onRefresh };
  }

  it('names the server, refreshes on open and follows the display rules', () => {
    const { renderer, onRefresh } = render();
    expect(onRefresh).toHaveBeenCalledTimes(1);
    const all = texts(renderer);
    expect(all).toContain('Accounts signed in on Studio Mac');
    expect(all).toContain('CLAUDE');
    expect(all).toContain('Max');
    expect(all).toContain('Session');
    expect(all).toContain('42%');
    expect(all).toContain('Extra usage');
    expect(all).toContain('$1.25 / $50.00');
    expect(all).not.toContain('Fable'); // scoped, inactive, under 0.5 %
    expect(all).not.toContain('CODEX'); // disabled
    expect(all.some((t) => t.startsWith('resets in 2h 30m'))).toBe(true);
    expect(all).toContain('Updated 2m ago');

    const row = renderer.root.find(
      (n) => n.props.testID === 'usage-window-five_hour' && n.props.accessible === true,
    );
    expect(row.props.accessibilityLabel).toMatch(/^Session, 5h window, 42% used, 50% of the time elapsed/);
  });

  it('shows the pace note when usage runs ahead of time', () => {
    const fast: UsageInfo = {
      ...usage,
      providers: [
        { ...usage.providers[0]!, windows: [{ ...usage.providers[0]!.windows[0]!, usedPercent: 70 }] },
      ],
    };
    const all = texts(render({ usage: fast }).renderer);
    expect(all).toContain('Slow down to last the period');
  });

  it('shows a Grok section with its prepaid balance', () => {
    const grok: UsageInfo = {
      ...usage,
      providers: [
        {
          provider: 'grok',
          status: 'ok',
          fetching: false,
          consecutiveFailures: 0,
          prepaidBalance: 15.5,
          windows: [
            {
              id: 'included',
              label: 'Credits',
              kind: 'other',
              usedPercent: 40,
              windowSeconds: 2_592_000,
              resetAt: now + 864_000,
              active: true,
              history: [],
            },
          ],
        },
      ],
    };
    const all = texts(render({ usage: grok }).renderer);
    expect(all).toContain('GROK');
    expect(all).toContain('Credits');
    expect(all).toContain('40%');
    expect(all).toContain('Prepaid');
    expect(all).toContain('15.50');
  });

  it("shows Claude's account pool instead of the login's windows", () => {
    const pooled: UsageInfo = {
      ...usage,
      providers: [{ ...usage.providers[0]!, pool: samplePool(now, 3 * 3_600) }],
    };
    const { renderer } = render({ usage: pooled });
    const all = texts(renderer);
    expect(all).toContain('CLAUDE POOL');
    expect(all).not.toContain('Session'); // the login's window
    expect(all).not.toContain('Extra usage');
    expect(all).toContain('Today');
    expect(all).toContain('3h00m left');
    expect(all.some((t) => t.startsWith('Work up to this and every working day'))).toBe(true);
    expect(all).toContain('Used today');
    expect(all).toContain('1h02m (4%)');
    expect(all).toContain('Safe per working day');
    expect(all).toContain("At the last 24h's pace");
    expect(all).toContain('lasts the week');
    expect(all).toContain('Nonstop');
    expect(all).toContain('blocked');
    expect(all).toContain('Every account has hit its 5h or weekly limit; the first is back today 11:10.');
    expect(all).toContain('Work');
    expect(all).toContain('Max 5x');
    expect(all).toContain('7d limit');
    expect(all).toEqual(expect.arrayContaining(['5h', '30%', '7d', '100%', 'today +2%']));
    expect(all).toContain('resets tomorrow 11:00');
  });

  it('explains an empty snapshot and a missing one', () => {
    const off: UsageInfo = { ...usage, providers: [usage.providers[1]!] };
    expect(texts(render({ usage: off }).renderer).join(' ')).toMatch(/turned off on Studio Mac/);
    expect(texts(render({ usage: null }).renderer).join(' ')).toMatch(/No usage data from Studio Mac yet/);
  });
});
