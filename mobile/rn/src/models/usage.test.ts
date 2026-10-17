/// <reference types="jest" />
/// <reference types="node" />
// Pin the zone before any Date use: the calendar math is local-time, like the
// Rust tests this mirrors (`crates/okena-agent-usage/src/pace.rs`).
process.env.TZ = 'Europe/Prague';

import type { ProviderUsageInfo, UsageInfo, UsageWindowInfo } from '../native/okena';
import {
  ALL_WORKING_DAYS,
  buildUsageView,
  formatDuration,
  formatResetClock,
  formatResetsIn,
  formatWindowLabel,
  hasHistoryShape,
  headlineSeverity,
  indicatorAccessibilityLabel,
  indicatorItems,
  linearTimePct,
  resetAlignedSegments,
  segmentUnitForWindow,
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
