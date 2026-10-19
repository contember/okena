/// <reference types="jest" />
/// <reference types="node" />
// Pin the zone before any Date use: the calendar math is local-time, like the
// Rust tests this mirrors (`crates/okena-agent-usage/src/pace.rs`).
process.env.TZ = 'Europe/Prague';

import type { PoolInfo, ProviderUsageInfo, UsageInfo, UsageWindowInfo } from '../native/okena';
import {
  absSeverity,
  ALL_WORKING_DAYS,
  buildUsageView,
  formatBalance,
  formatDuration,
  formatPoolAmount,
  grokPeriodLabel,
  formatResetClock,
  formatResetsIn,
  formatWindowLabel,
  hasHistoryShape,
  headlineSeverity,
  indicatorAccessibilityLabel,
  indicatorItems,
  linearTimePct,
  paceSeverity,
  poolNonstopSeverity,
  poolNonstopValue,
  poolTodaySeverity,
  poolTodayValue,
  resetAlignedSegments,
  segmentUnitForWindow,
  shortDuration,
  timeFraction,
  workingDayReshape,
  workingDaysFromIndices,
} from './usage';

const DAY = 86_400;
const WEEK = 7 * DAY;
const MON_FRI = workingDaysFromIndices([0, 1, 2, 3, 4]);

function win(overrides: Partial<UsageWindowInfo>): UsageWindowInfo {
  return {
    id: 'five_hour',
    label: 'Session',
    kind: 'five_hour',
    usedPercent: 20,
    windowSeconds: 18_000,
    active: true,
    history: [],
    ...overrides,
  };
}

function provider(overrides: Partial<ProviderUsageInfo>): ProviderUsageInfo {
  return {
    provider: 'claude',
    status: 'ok',
    fetching: false,
    consecutiveFailures: 0,
    windows: [],
    ...overrides,
  };
}

function usage(providers: ProviderUsageInfo[], workingDays = [0, 1, 2, 3, 4, 5, 6]): UsageInfo {
  return { providers, workingDays, generatedAt: 0 };
}

describe('pace math (port of pace.rs)', () => {
  it('linear time pct', () => {
    expect(linearTimePct(150, 100, 100)).toBe(50);
    expect(linearTimePct(150, 100, 10)).toBe(0);
    expect(linearTimePct(150, 100, 500)).toBe(100);
    expect(linearTimePct(150, 0, 100)).toBeUndefined();
  });

  it('headline severity takes the worse of abs and pace', () => {
    expect(headlineSeverity(65, 30)).toBe('critical');
    expect(headlineSeverity(65, 64)).toBe('warning');
    expect(headlineSeverity(20, 16)).toBe('normal');
    expect(headlineSeverity(20, 2)).toBe('critical');
    expect(headlineSeverity(85, undefined)).toBe('critical');
    expect(headlineSeverity(12, 5)).toBe('warning');
  });

  it('working days parse like the wire (empty or invalid = every day)', () => {
    expect(workingDaysFromIndices([])).toEqual(ALL_WORKING_DAYS);
    expect(workingDaysFromIndices([9])).toEqual(ALL_WORKING_DAYS);
    expect(workingDaysFromIndices([0, 9, 3])).toEqual([true, false, false, true, false, false, false]);
  });

  it('all days keeps the calendar grid', () => {
    const seg = resetAlignedSegments(1_700_000_000, WEEK, 'day', ALL_WORKING_DAYS, 1_699_800_000);
    expect(seg.timePct).toBeUndefined();
    expect(seg.dividers.length).toBeGreaterThanOrEqual(5);
    expect(seg.dividers.length).toBeLessThanOrEqual(7);
    for (const f of seg.dividers) expect(f > 0 && f < 1).toBe(true);
    for (let i = 1; i < seg.dividers.length; i++) {
      expect(seg.dividers[i]!).toBeGreaterThan(seg.dividers[i - 1]!);
    }
  });

  it('five working days reshape to five blocks', () => {
    const seg = resetAlignedSegments(1_700_000_000, WEEK, 'day', MON_FRI, 1_699_800_000);
    expect(seg.dividers).toHaveLength(4);
    seg.dividers.forEach((d, i) => expect(d).toBeCloseTo((i + 1) / 5, 4));
    expect(seg.timePct).toBeDefined();
    expect(seg.timePct!).toBeGreaterThanOrEqual(0);
    expect(seg.timePct!).toBeLessThanOrEqual(100);
  });

  it('reshape marker tracks the calendar day, not the reset time', () => {
    const reset = 1_781_953_200; // 2026-06-20T11:00:00Z (Sat)
    const now = 1_781_773_200; // 2026-06-18T09:00:00Z (Thu)
    const seg = workingDayReshape(reset - WEEK, reset, MON_FRI, now)!;
    expect(seg.dividers).toEqual([0.2, 0.4, 0.6, 0.8]);
    expect(seg.current![0]).toBeCloseTo(0.6, 4);
    expect(seg.current![1]).toBeCloseTo(0.8, 4);
    expect(seg.timePct!).toBeGreaterThanOrEqual(60);
    expect(seg.timePct!).toBeLessThan(80);
  });

  it('reshape on an off-day pegs to the boundary', () => {
    const reset = 1_781_953_200;
    const now = 1_781_946_000; // 2026-06-20T09:00:00Z (Sat)
    const seg = workingDayReshape(reset - WEEK, reset, MON_FRI, now)!;
    expect(seg.current).toBeUndefined();
    expect(seg.timePct).toBe(100);
  });

  it('working-time fraction freezes over the weekend', () => {
    const reset = 1_781_953_200;
    const seg = workingDayReshape(reset - WEEK, reset, MON_FRI, 1_781_946_000)!;
    // Saturday morning and Saturday noon sit on the same boundary.
    expect(timeFraction(seg, 1_781_946_000, reset, WEEK)).toBe(1);
    expect(timeFraction(seg, 1_781_950_000, reset, WEEK)).toBe(1);
  });

  it('hour windows never reshape', () => {
    const seg = resetAlignedSegments(1_700_000_000, 5 * 3600, 'hour', MON_FRI, 1_699_990_000);
    expect(seg.timePct).toBeUndefined();
    expect(seg.dividers).toHaveLength(4);
    expect(seg.current).toBeDefined();
  });

  it('guards reject bad input', () => {
    expect(resetAlignedSegments(0, WEEK, 'day', ALL_WORKING_DAYS, 0).dividers).toEqual([]);
    expect(resetAlignedSegments(1_700_000_000, 0, 'day', ALL_WORKING_DAYS, 0).dividers).toEqual([]);
  });

  it('window units and labels', () => {
    expect(segmentUnitForWindow(0)).toBeUndefined();
    expect(segmentUnitForWindow(3600)).toBeUndefined();
    expect(segmentUnitForWindow(5 * 3600)).toBe('hour');
    expect(segmentUnitForWindow(DAY)).toBe('hour');
    expect(segmentUnitForWindow(WEEK)).toBe('day');
    expect(formatWindowLabel(18_000)).toBe('5h');
    expect(formatWindowLabel(WEEK)).toBe('7d');
  });
});

describe('formatting', () => {
  // 2026-06-18 (Thu) 11:00 local (09:00Z).
  const now = 1_781_773_200;

  it('formats durations and "resets in"', () => {
    expect(formatDuration(42)).toBe('42s');
    expect(formatDuration(12 * 60 + 5)).toBe('12m');
    expect(formatDuration(2 * 3600 + 14 * 60)).toBe('2h 14m');
    expect(formatDuration(3 * DAY + 4 * 3600)).toBe('3d 4h');
    expect(formatResetsIn(now + 2 * 3600 + 14 * 60, now)).toBe('resets in 2h 14m');
    expect(formatResetsIn(now + 30, now)).toBe('resets in <1m');
    expect(formatResetsIn(now - 1, now)).toBe('resetting now');
  });

  it('formats the local reset clock with smart dates', () => {
    expect(formatResetClock(now + 3 * 3600, now, false)).toBe('14:00');
    expect(formatResetClock(now + 3 * 3600, now, true)).toBe('today 14:00');
    expect(formatResetClock(now + DAY, now, true)).toBe('tomorrow 11:00');
    expect(formatResetClock(now + 2 * DAY + 2 * 3600, now, true)).toBe('Sat 13:00');
    expect(formatResetClock(now + 10 * DAY, now, true)).toBe('Jun 28, 11:00');
    expect(formatResetClock(0, now, true)).toBe('');
  });
});

describe('display rules', () => {
  const now = 1_781_773_200;

  it('hides disabled and unknown providers', () => {
    const view = buildUsageView(
      usage([
        provider({ provider: 'claude', status: 'disabled' }),
        provider({ provider: 'codex', status: 'pending' }),
        provider({ provider: 'unknown', status: 'ok' }),
      ]),
      now,
    );
    expect(view.map((p) => p.provider)).toEqual(['codex']);
    expect(view[0]!.statusText).toBe('Waiting for the first update…');
  });

  it('shows scoped Claude windows only when binding or >= 0.5%', () => {
    const scoped = (model: string, usedPercent: number, active: boolean) =>
      win({
        id: `seven_day_${model.toLowerCase()}`,
        label: model,
        kind: 'weekly_scoped',
        scopedModel: model,
        usedPercent,
        windowSeconds: WEEK,
        active,
      });
    const view = buildUsageView(
      usage([
        provider({
          plan: 'max',
          windows: [
            win({ resetAt: now + 3600 }),
            win({ id: 'seven_day', label: 'Weekly', kind: 'weekly', windowSeconds: WEEK }),
            scoped('Fable', 0.2, false),
            scoped('Opus', 0.6, false),
            scoped('Sonnet', 0, true),
          ],
        }),
      ]),
      now,
    );
    const claude = view[0]!;
    expect(claude.plan).toBe('Max');
    expect(claude.windows.map((w) => w.title)).toEqual(['Session', 'Weekly', 'Opus', 'Sonnet']);
    expect(claude.windows[0]!.resetsIn).toBe('resets in 1h 0m');
    expect(claude.windows[0]!.period).toBe('5h');
  });

  it('labels Codex primary / secondary from the window length', () => {
    const view = buildUsageView(
      usage([
        provider({
          provider: 'codex',
          windows: [
            win({ id: 'primary', label: 'Rate Limit', kind: 'five_hour' }),
            win({ id: 'secondary', label: 'Secondary', kind: 'weekly', windowSeconds: WEEK }),
            win({ id: 'code_review', label: 'Code Review', kind: 'code_review', windowSeconds: WEEK }),
            win({ id: 'mystery', label: 'Mystery', kind: 'other' }),
          ],
          credits: { hasCredits: true, unlimited: false, balance: 4.5 },
          resetCredits: { availableCount: 2, credits: [{ title: 'Reset', expiresAt: now + 3600 }] },
        }),
      ]),
      now,
    );
    const codex = view[0]!;
    expect(codex.windows.map((w) => w.title)).toEqual(['5-hour limit', 'Weekly limit', 'Code review']);
    expect(codex.credits).toEqual({ text: '$4.50', unlimited: false });
    expect(codex.resetCredits?.summary).toBe('2 resets available');
    expect(codex.resetCredits?.firstExpiry).toBe('first expires today 12:00');
  });

  it('reports Claude extra usage only when enabled', () => {
    const extra = { isEnabled: true, monthlyLimitCents: 10_000, usedCreditsCents: 250, utilization: 2.5 };
    const on = buildUsageView(usage([provider({ extraUsage: extra })]), now)[0]!;
    expect(on.extra).toEqual({ used: '$2.50', limit: '$100.00', utilization: 2.5 });
    const off = buildUsageView(usage([provider({ extraUsage: { ...extra, isEnabled: false } })]), now)[0]!;
    expect(off.extra).toBeUndefined();
  });

  it('puts the pace marker and history on the working-time axis for weekly windows', () => {
    const reset = 1_781_953_200; // Sat
    const weekly = win({
      id: 'seven_day',
      kind: 'weekly',
      windowSeconds: WEEK,
      resetAt: reset,
      usedPercent: 50,
      history: [
        { recordedAt: reset - WEEK + 3600, usedPercent: 0 },
        { recordedAt: now, usedPercent: 50 },
      ],
    });
    const linear = buildUsageView(usage([provider({ windows: [weekly] })]), now)[0]!.windows[0]!;
    const reshaped = buildUsageView(usage([provider({ windows: [weekly] })], [0, 1, 2, 3, 4]), now)[0]!
      .windows[0]!;
    expect(linear.timePct!).toBeCloseTo(linearTimePct(reset, WEEK, now)!, 6);
    expect(reshaped.timePct!).toBeGreaterThanOrEqual(60);
    expect(reshaped.segments.dividers).toHaveLength(4);
    expect(reshaped.history[1]!.x).toBeCloseTo(reshaped.timePct! / 100, 6);
    expect(reshaped.history[1]!.y).toBe(0.5);
    expect(reshaped.resetClock).toBe('Sat 13:00');
  });

  it('explains errors and missing credentials', () => {
    const view = buildUsageView(
      usage([
        provider({ status: 'error', error: 'HTTP 500', fetchedAt: now - 600, nextAttemptAt: now + 30 }),
        provider({ provider: 'codex', status: 'no_credentials' }),
      ]),
      now,
    );
    expect(view[0]!.statusText).toBe('Update failed: HTTP 500');
    expect(view[0]!.detailText).toBe('Updated 10m ago · retry in 30s');
    expect(view[0]!.isError).toBe(true);
    expect(view[1]!.statusText).toBe('Not signed in on this machine');
  });
});

describe('Grok', () => {
  const now = 1_781_773_200;
  const credits = (id: string, label: string, usedPercent: number) =>
    win({ id, label, kind: 'other', usedPercent, windowSeconds: 30 * DAY, resetAt: now + 10 * DAY });

  it('titles the credit windows and shows prepaid only above zero', () => {
    const grok = (prepaidBalance?: number) =>
      buildUsageView(
        usage([
          provider({
            provider: 'grok',
            windows: [credits('included', 'Credits', 40), credits('on_demand', 'On-demand', 5)],
            prepaidBalance,
          }),
        ]),
        now,
      )[0]!;
    const view = grok(15.5);
    expect(view.name).toBe('Grok');
    expect(view.windows.map((w) => [w.title, w.period])).toEqual([
      ['Credits', '30d'],
      ['On-demand', '30d'],
    ]);
    expect(view.prepaid).toBe('15.50');
    expect(grok(0).prepaid).toBeUndefined();
    expect(grok(undefined).prepaid).toBeUndefined();
  });

  it("labels periods like the desktop's Grok widget", () => {
    expect(grokPeriodLabel(0)).toBe('period');
    expect(grokPeriodLabel(14 * DAY)).toBe('14d');
    expect(grokPeriodLabel(30 * DAY)).toBe('30d');
    const view = buildUsageView(
      usage([provider({ provider: 'grok', windows: [credits('included', 'Credits', 40)] })]),
      now,
    )[0]!;
    expect(view.windows[0]!.period).toBe('30d');
    const fortnight = buildUsageView(
      usage([
        provider({
          provider: 'grok',
          windows: [win({ id: 'included', kind: 'other', windowSeconds: 14 * DAY, resetAt: now + DAY })],
        }),
      ]),
      now,
    )[0]!;
    expect(fortnight.windows[0]!.period).toBe('14d');
  });

  it('formats balances like the desktop', () => {
    expect(formatBalance(15)).toBe('15');
    expect(formatBalance(15.5)).toBe('15.50');
  });

  it('headlines the worse of credits and on-demand', () => {
    const items = indicatorItems(
      usage([
        provider({
          provider: 'grok',
          windows: [credits('included', 'Credits', 10), credits('on_demand', 'On-demand', 90)],
        }),
      ]),
      now,
    );
    expect(items).toEqual([
      { provider: 'grok', name: 'Grok', usedPercent: 90, severity: 'critical', period: '30d' },
    ]);
  });
});

describe('Claude account pool', () => {
  const now = 1_781_773_200; // Thu 2026-06-18 11:00 CEST
  const amount = (workSeconds: number, accountWeekPercent: number) => ({ workSeconds, accountWeekPercent });

  function pool(overrides: Partial<PoolInfo> = {}): PoolInfo {
    return {
      today: {
        left: amount(3 * 3600, 3),
        used: amount(3720, 4),
        safePerDay: amount(4 * 3600 + 3720, 7),
        offDay: amount(1800, 1),
      },
      nonstop: { kind: 'lasts', workSeconds: 26 * 3600 },
      pace: { sessionFillSeconds: 2 * 3600 + 600, sessionWeekPercent: 9.4, fromHistory: true },
      accounts: [
        {
          id: 'a1',
          label: 'Work',
          tier: 'Max 5x',
          blocked: '5h limit',
          usedTodayPercent: 3.6,
          windows: [
            win({ usedPercent: 100, resetAt: now + 3600 }),
            win({ id: 'seven_day', label: 'Weekly', kind: 'weekly', windowSeconds: WEEK, usedPercent: 40, resetAt: now + 2 * DAY }),
          ],
        },
      ],
      ...overrides,
    };
  }

  it('formats short work durations like short_duration', () => {
    expect(shortDuration(0)).toBe('1m');
    expect(shortDuration(45 * 60)).toBe('45m');
    expect(shortDuration(45 * 60 + 59)).toBe('45m');
    expect(shortDuration(3 * 3600 + 600)).toBe('3h10m');
    expect(shortDuration(3600 + 120)).toBe('1h02m');
    expect(shortDuration(26 * 3600)).toBe('26h');
    expect(shortDuration(7 * DAY)).toBe('7d');
    expect(shortDuration(600 * 60 - 1)).toBe('9h59m');
    expect(shortDuration(600 * 60)).toBe('10h');
    expect(shortDuration(2880 * 60 - 1)).toBe('47h');
    expect(shortDuration(2880 * 60)).toBe('2d');
    expect(shortDuration(-90)).toBe('1m');
    expect(formatPoolAmount(amount(3720, 4.4))).toBe('1h02m (4%)');
  });

  it('says how much today has left or went over, with its severity', () => {
    expect(poolTodayValue(amount(3 * 3600, 3))).toBe('3h00m left');
    expect(poolTodayValue(amount(-45 * 60, -1))).toBe('45m over');
    expect(poolTodaySeverity(amount(3600, 1))).toBe('normal');
    expect(poolTodaySeverity(amount(3599, 1))).toBe('warning');
    expect(poolTodaySeverity(amount(0, 0))).toBe('warning');
    expect(poolTodaySeverity(amount(-1, 0))).toBe('critical');
  });

  it('says how long nonstop work lasts, with its severity', () => {
    const lasts = (workSeconds: number) => ({ kind: 'lasts', workSeconds });
    expect(poolNonstopValue(lasts(26 * 3600))).toBe('26h');
    expect(poolNonstopValue({ kind: 'beyond_horizon', horizonSeconds: WEEK })).toBe('a week or more');
    expect(poolNonstopValue({ kind: 'blocked', until: now + 600 })).toBe('blocked');
    expect(poolNonstopValue({ kind: 'unknown' })).toBe('–');

    expect(poolNonstopSeverity(lasts(3 * 3600))).toBe('normal');
    expect(poolNonstopSeverity(lasts(3 * 3600 - 1))).toBe('warning');
    expect(poolNonstopSeverity(lasts(3599))).toBe('critical');
    expect(poolNonstopSeverity({ kind: 'blocked' })).toBe('critical');
    expect(poolNonstopSeverity({ kind: 'beyond_horizon', horizonSeconds: WEEK })).toBe('normal');
    expect(poolNonstopSeverity({ kind: 'unknown' })).toBe('normal');
  });

  it('replaces the login rows and explains Today and Nonstop', () => {
    const login = provider({
      plan: 'max',
      status: 'no_credentials',
      windows: [win({ resetAt: now + 3600 })],
      extraUsage: { isEnabled: true, monthlyLimitCents: 100, usedCreditsCents: 1, utilization: 1 },
      pool: pool({
        today: {
          ...pool().today,
          runsShortAt: now + 2 * DAY,
          runsShortBefore: 'Work',
          runsOutAtRecentPace: now + DAY,
        },
      }),
    });
    const view = buildUsageView(usage([login], [0, 1, 2, 3, 4]), now)[0]!;
    expect(view.windows).toEqual([]);
    expect(view.plan).toBeUndefined();
    expect(view.extra).toBeUndefined();
    expect(view.statusText).toBeUndefined();
    const p = view.pool!;
    expect(p.todayValue).toBe('3h00m left');
    expect(p.todaySeverity).toBe('normal');
    expect(p.todayNotes).toEqual([
      'Work up to this and every working day until the resets still has quota. ' +
        'More is possible, but the pool then runs short Sat 11:00, before Work resets.',
      'Times are work at your pace: a 5h window fills in 2h10m and costs 9% of a week. ' +
        "Percentages are of one account's week. Measured over the last 7 days.",
    ]);
    expect(p.usedToday).toBe('1h02m (4%)');
    expect(p.safePerDay).toBe('5h02m (7%)');
    expect(p.recentPace).toEqual({ text: 'runs out tomorrow 11:00', warning: true });
    expect(p.offDayNote).toBe('Days off use 30m (1%) each, as last week.');
    expect(p.nonstopValue).toBe('26h');
    expect(p.nonstopSeverity).toBe('normal');
    expect(p.nonstopNote).toBe(
      'Working nonstop from now, every account hits its 5h or weekly limit after this long. ' +
        'Accounts come back as their windows reset.',
    );

    const account = p.accounts[0]!;
    expect(account).toMatchObject({ label: 'Work', tier: 'Max 5x', blocked: '5h limit' });
    expect(account.windows.map((w) => [w.title, Math.round(w.usedPercent), w.severity])).toEqual([
      ['5h', 100, 'critical'],
      ['7d', 40, 'normal'],
    ]);
    expect(account.usedToday).toBe('today +4%');
    expect(account.weeklyReset).toBe('resets Sat 11:00');
  });

  it('falls back to the plain explanations without a bottleneck or recent-pace wall', () => {
    const blocked = pool({
      nonstop: { kind: 'blocked', until: now + 600 },
      pace: { sessionFillSeconds: 7200, sessionWeekPercent: 10, fromHistory: false },
    });
    const p = buildUsageView(usage([provider({ pool: blocked })]), now)[0]!.pool!;
    expect(p.todayNotes[0]).toBe(
      'Work up to this and every working day until the resets still has quota. ' +
        'More takes from what the pool refills for later weeks.',
    );
    expect(p.todayNotes[1]).toMatch(/Defaults until there is more history\.$/);
    expect(p.recentPace).toEqual({ text: 'lasts the week', warning: false });
    expect(p.offDayNote).toBeUndefined(); // every day is a working day
    expect(p.nonstopNote).toBe('Every account has hit its 5h or weekly limit; the first is back today 11:10.');
    const unknownUntil = buildUsageView(
      usage([provider({ pool: pool({ nonstop: { kind: 'blocked' } }) })]),
      now,
    )[0]!.pool!;
    expect(unknownUntil.nonstopNote).toBe('Every account has hit its 5h or weekly limit.');
  });

  it('has no Nonstop note for a state from a newer daemon', () => {
    const p = buildUsageView(usage([provider({ pool: pool({ nonstop: { kind: 'unknown' } }) })]), now)[0]!
      .pool!;
    expect(p.nonstopValue).toBe('–');
    expect(p.nonstopNote).toBeUndefined();
  });

  it('shows the pool even when the Claude login is disabled', () => {
    const view = buildUsageView(usage([provider({ status: 'disabled', pool: pool() })]), now);
    expect(view.map((p) => p.provider)).toEqual(['claude']);
    expect(view[0]!.pool?.todayValue).toBe('3h00m left');
  });

  it('hides the days-off line when days off use nothing', () => {
    const idle = pool({ today: { ...pool().today, offDay: amount(0, 0) } });
    const p = buildUsageView(usage([provider({ pool: idle })], [0, 1, 2, 3, 4]), now)[0]!.pool!;
    expect(p.offDayNote).toBeUndefined();
  });

  it("puts the pool's Today in the indicator instead of the login's windows", () => {
    const items = indicatorItems(
      usage([
        provider({
          status: 'no_credentials',
          windows: [win({ usedPercent: 90 })],
          pool: pool({ today: { ...pool().today, left: amount(-45 * 60, -1) } }),
        }),
      ]),
      now,
    );
    expect(items).toEqual([{ provider: 'claude', name: 'Claude', severity: 'critical', today: '45m over' }]);
    expect(indicatorAccessibilityLabel(items)).toBe('Agent usage: Claude pool 45m over today, critical');
  });
});

describe('history sparkline', () => {
  it('needs two samples spanning at least 3% of the bar', () => {
    expect(hasHistoryShape([{ x: 0.5 }])).toBe(false);
    expect(hasHistoryShape([{ x: 0.5 }, { x: 0.51 }])).toBe(false);
    expect(hasHistoryShape([{ x: 0.1 }, { x: 0.2 }, { x: 0.5 }])).toBe(true);
  });
});

describe('indicator', () => {
  const now = 1_781_773_200;

  it('is empty when both providers are off or signed out', () => {
    expect(indicatorItems(null, now)).toEqual([]);
    expect(
      indicatorItems(
        usage([
          provider({ status: 'disabled' }),
          provider({ provider: 'codex', status: 'no_credentials' }),
        ]),
        now,
      ),
    ).toEqual([]);
  });

  it('headlines the session / primary window unless another is worse', () => {
    const items = indicatorItems(
      usage([
        provider({
          windows: [
            win({ usedPercent: 20, resetAt: now + 9000 }), // half-way through 5h: on pace
            win({ id: 'seven_day', kind: 'weekly', windowSeconds: WEEK, usedPercent: 90 }),
          ],
        }),
        provider({
          provider: 'codex',
          windows: [win({ id: 'primary', kind: 'five_hour', usedPercent: 3, resetAt: now + 9000 })],
        }),
      ]),
      now,
    );
    expect(items).toEqual([
      { provider: 'claude', name: 'Claude', usedPercent: 90, severity: 'critical', period: '7d' },
      { provider: 'codex', name: 'Codex', usedPercent: 3, severity: 'normal', period: '5h' },
    ]);
    expect(indicatorAccessibilityLabel(items)).toBe(
      'Agent usage: Claude 90% of 7d limit, critical; Codex 3% of 5h limit',
    );
  });

  it('keeps a pending provider as a no-data item', () => {
    const items = indicatorItems(usage([provider({ status: 'pending' })]), now);
    expect(items).toEqual([{ provider: 'claude', name: 'Claude', usedPercent: undefined, severity: 'normal', period: undefined }]);
    expect(indicatorAccessibilityLabel(items)).toBe('Agent usage: Claude no data yet');
  });
});

// Same inputs and numbers as `golden_values_shared_with_mobile_port` and
// `golden_dst_week_in_prague_inner` in `crates/okena-agent-usage/src/pace.rs`.
// Change both together. The zone is the `process.env.TZ` pinned at the top;
// the Rust side runs that case in a child process with `TZ=Europe/Prague`.
describe('golden values shared with pace.rs', () => {
  it('runs in Europe/Prague', () => {
    expect(new Date(Date.UTC(2026, 6, 1)).getTimezoneOffset()).toBe(-120);
    expect(new Date(Date.UTC(2026, 11, 1)).getTimezoneOffset()).toBe(-60);
  });

  it('matches linear time and severities', () => {
    expect(linearTimePct(1_000, 18_000, -3_500)).toBe(75);
    expect(linearTimePct(1_000, 18_000, -20_000)).toBe(0);
    expect(linearTimePct(1_000, 18_000, 2_000)).toBe(100);
    expect(linearTimePct(1_000, -1, 0)).toBeUndefined();

    const cases: [number, number | undefined, string, string, string][] = [
      [60, 55, 'normal', 'normal', 'normal'],
      [60.5, 55, 'warning', 'warning', 'warning'],
      [80, 64, 'warning', 'critical', 'critical'],
      [80.5, 90, 'critical', 'normal', 'critical'],
      [40, 25, 'normal', 'warning', 'warning'],
      [40, 24.9, 'normal', 'critical', 'critical'],
      [95, undefined, 'critical', 'normal', 'critical'],
    ];
    for (const [pct, timePct, abs, pace, headline] of cases) {
      expect([absSeverity(pct), paceSeverity(pct, timePct), headlineSeverity(pct, timePct)]).toEqual([
        abs,
        pace,
        headline,
      ]);
    }

    const labels: [number, string][] = [
      [3_600, '1h'],
      [3_601, '5h'],
      [18_000, '5h'],
      [86_400, '1d'],
      [604_800, '7d'],
      [2_592_000, '30d'],
    ];
    for (const [secs, label] of labels) expect(formatWindowLabel(secs)).toBe(label);
  });

  describe('a week across the DST end (Sun 2026-10-25)', () => {
    const reset = 1_793_178_000; // 2026-10-28T09:00:00Z (Wed 10:00 CET)
    const start = reset - WEEK; // 2026-10-21T09:00:00Z (Wed 11:00 CEST)

    it('anchors the plain grid to the local reset time', () => {
      const grid = resetAlignedSegments(reset, WEEK, 'day', ALL_WORKING_DAYS, start);
      const expected = [0.13690476, 0.2797619, 0.42261905, 0.5714286, 0.7142857, 0.8571429];
      expect(grid.dividers).toHaveLength(expected.length);
      grid.dividers.forEach((d, i) => expect(d).toBeCloseTo(expected[i]!, 6));
      expect(grid.timePct).toBeUndefined();
    });

    it('reshapes Mon–Fri to five blocks on a working day', () => {
      const monday = 1_793_023_200; // 2026-10-26T14:00:00Z (Mon 15:00 CET)
      const seg = workingDayReshape(start, reset, MON_FRI, monday)!;
      expect(seg.dividers).toEqual([0.2, 0.4, 0.6, 0.8]);
      expect(seg.current).toEqual([0.6, 0.8]);
      expect(seg.timePct).toBeCloseTo(72.5, 4);
      expect(timeFraction(seg, monday, reset, WEEK)).toBeCloseTo(0.725, 6);
    });

    it('pegs an off-day to the last elapsed working day', () => {
      const sunday = 1_792_926_000; // 2026-10-25T11:00:00Z (Sun 12:00 CET)
      const seg = workingDayReshape(start, reset, MON_FRI, sunday)!;
      expect(seg.current).toBeUndefined();
      expect(seg.timePct).toBe(60);
    });

    it('counts the 25-hour Sunday from local midnight', () => {
      const evening = 1_792_954_800; // 2026-10-25T19:00:00Z (Sun 20:00 CET, 21 h in)
      const seg = workingDayReshape(start, reset, workingDaysFromIndices([0, 1, 2, 3, 4, 6]), evening)!;
      expect(seg.dividers).toHaveLength(5);
      expect(seg.current![0]).toBeCloseTo(0.5, 6);
      expect(seg.current![1]).toBeCloseTo(4 / 6, 6);
      expect(seg.timePct).toBeCloseTo(64.583333, 4);
    });
  });
});
