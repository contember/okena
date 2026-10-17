/**
 * usage.ts — Claude Code / Codex usage: pace, severity and display rules.
 *
 * A port of `crates/okena-agent-usage/src/pace.rs` (the math the desktop
 * widgets use) plus the display rules of the desktop popovers
 * (`okena-ext-claude` / `okena-ext-codex`), so the phone shows the same
 * numbers and colors as the desktop for the same snapshot.
 *
 * Time is epoch SECONDS (the wire unit). Calendar math (working-day blocks,
 * reset-anchored dividers, "today/tomorrow" labels) uses the device's local
 * time zone through `Date`, like the desktop uses the system zone.
 */

import type {
  ProviderUsageInfo,
  ResetCreditsInfo,
  UsageInfo,
  UsageWindowInfo,
} from '../native/okena';

// ── Severity ─────────────────────────────────────────────────────────────

export type Severity = 'normal' | 'warning' | 'critical';

const SEVERITY_RANK: Record<Severity, number> = { normal: 0, warning: 1, critical: 2 };

export function maxSeverity(a: Severity, b: Severity): Severity {
  return SEVERITY_RANK[a] >= SEVERITY_RANK[b] ? a : b;
}

/** Linear time-elapsed percentage (0–100) of a window, or `undefined`. */
export function linearTimePct(resetAt: number, periodSecs: number, now: number): number | undefined {
  if (periodSecs <= 0 || !Number.isFinite(resetAt)) return undefined;
  const remaining = Math.max(resetAt - now, 0);
  const elapsed = Math.max(periodSecs - remaining, 0);
  return clamp((elapsed / periodSecs) * 100, 0, 100);
}

/** Severity from absolute utilization: how close to the hard cap. */
export function absSeverity(pct: number): Severity {
  if (pct > 80) return 'critical';
  if (pct > 60) return 'warning';
  return 'normal';
}

/** Severity from pace: how far usage runs ahead of the time elapsed. */
export function paceSeverity(usagePct: number, timePct: number | undefined): Severity {
  if (timePct === undefined) return 'normal';
  if (usagePct > timePct + 15) return 'critical';
  if (usagePct > timePct + 5) return 'warning';
  return 'normal';
}

/** The worse of nearness-to-cap and burn pace (the desktop's headline color). */
export function headlineSeverity(pct: number, timePct: number | undefined): Severity {
  return maxSeverity(absSeverity(pct), paceSeverity(pct, timePct));
}

// ── Working days ─────────────────────────────────────────────────────────

/** `days[0]` = Monday … `days[6]` = Sunday. */
export type WorkingDays = readonly boolean[];

export const ALL_WORKING_DAYS: WorkingDays = [true, true, true, true, true, true, true];

/** Wire indices (`0 = Monday`) → flags. Empty or all-invalid means every day. */
export function workingDaysFromIndices(indices: readonly number[]): WorkingDays {
  const days = [false, false, false, false, false, false, false];
  for (const i of indices) if (Number.isInteger(i) && i >= 0 && i < 7) days[i] = true;
  return days.some(Boolean) ? days : ALL_WORKING_DAYS;
}

function workingCount(days: WorkingDays): number {
  return days.filter(Boolean).length;
}

// ── Local calendar helpers ───────────────────────────────────────────────

/** A local calendar date. */
interface CivilDate {
  y: number;
  m: number; // 0-based month, as `Date`
  d: number;
}

function localDate(epoch: number): CivilDate {
  const date = new Date(epoch * 1000);
  return { y: date.getFullYear(), m: date.getMonth(), d: date.getDate() };
}

/** Local midnight of a date, epoch seconds. */
function midnight(date: CivilDate): number {
  return new Date(date.y, date.m, date.d).getTime() / 1000;
}

function nextDay(date: CivilDate): CivilDate {
  const next = new Date(date.y, date.m, date.d + 1);
  return { y: next.getFullYear(), m: next.getMonth(), d: next.getDate() };
}

function dateKey(date: CivilDate): number {
  return date.y * 10000 + date.m * 100 + date.d;
}

/** `0 = Monday .. 6 = Sunday`. */
function mondayIndex(date: CivilDate): number {
  return (new Date(date.y, date.m, date.d).getDay() + 6) % 7;
}

// ── Segments (bar grid + working-time pace) ──────────────────────────────

export type SegmentUnit = 'hour' | 'day';

export interface Segments {
  /** Divider fractions in `(0, 1)`, excluding the ends. */
  dividers: number[];
  /** `[start, end]` fractions of the block containing "now". */
  current?: [number, number];
  /** Working-time elapsed percentage when the bar is reshaped to working days. */
  timePct?: number;
  /** Working calendar dates (keys) when reshaped. */
  workingDates: number[];
}

const EMPTY_SEGMENTS: Segments = { dividers: [], workingDates: [] };

/** Per-hour grid up to a day, per-day above; none for sub-hour windows. */
export function segmentUnitForWindow(windowSeconds: number): SegmentUnit | undefined {
  if (windowSeconds <= 3600) return undefined;
  if (windowSeconds <= 86400) return 'hour';
  return 'day';
}

/** Short period badge for a window length (`5h`, `7d`, …). */
export function formatWindowLabel(windowSeconds: number): string {
  const secs = Math.floor(Math.max(windowSeconds, 0));
  if (secs <= 3600) return '1h';
  if (secs <= 18000) return '5h';
  if (secs <= 86400) return '1d';
  if (secs <= 604800) return '7d';
  return '30d';
}

/**
 * Reset-anchored grid for a usage bar plus the block containing `now`. A
 * ~weekly window with a strict subset of working days is reshaped to one
 * equal block per working day, and `timePct` then advances on working time
 * only. Mirrors `reset_aligned_segments`.
 */
export function resetAlignedSegments(
  resetAt: number,
  periodSecs: number,
  unit: SegmentUnit,
  working: WorkingDays,
  now: number,
): Segments {
  if (periodSecs <= 0 || resetAt <= 0) return EMPTY_SEGMENTS;
  const start = resetAt - periodSecs;

  const dayUnits = Math.round(periodSecs / 86400);
  const count = workingCount(working);
  const remap = unit === 'day' && count >= 1 && count < 7 && dayUnits >= 2 && dayUnits <= 8;
  if (remap) {
    const reshaped = workingDayReshape(start, resetAt, working, now);
    if (reshaped) return reshaped;
  }

  const unitSecs = unit === 'hour' ? 3600 : 86400;
  const maxBlocks = unit === 'hour' ? 12 : 14;
  const units = Math.max(Math.round(periodSecs / unitSecs), 1);
  const multiplier = Math.max(Math.ceil(units / maxBlocks), 1);

  // Walk back from the reset one block at a time until the start is covered.
  // Hours are exact durations; days are calendar days (DST-safe).
  const bounds = [resetAt];
  const reset = new Date(resetAt * 1000);
  for (let k = 1; k <= 64; k++) {
    const epoch =
      unit === 'hour'
        ? resetAt - k * multiplier * 3600
        : new Date(
            reset.getFullYear(),
            reset.getMonth(),
            reset.getDate() - k * multiplier,
            reset.getHours(),
            reset.getMinutes(),
            reset.getSeconds(),
            reset.getMilliseconds(),
          ).getTime() / 1000;
    bounds.push(epoch);
    if (epoch <= start) break;
  }
  bounds.reverse();

  const frac = (epoch: number) => (epoch - start) / periodSecs;
  const dividers = bounds.map(frac).filter((f) => f > 0.001 && f < 0.999);
  let current: [number, number] | undefined;
  for (let i = 0; i + 1 < bounds.length; i++) {
    if (now >= bounds[i]! && now < bounds[i + 1]!) {
      current = [Math.max(frac(bounds[i]!), 0), Math.min(frac(bounds[i + 1]!), 1)];
      break;
    }
  }
  return { dividers, current, workingDates: [] };
}

/**
 * One equal block per working calendar day of a ~weekly window. Blocks are
 * keyed by local calendar date, so "today" is always today's block. Returns
 * `undefined` when no working day falls in the window. Mirrors
 * `working_day_reshape`.
 */
export function workingDayReshape(
  start: number,
  resetAt: number,
  working: WorkingDays,
  now: number,
): Segments | undefined {
  const startDate = localDate(start);
  const endKey = dateKey(localDate(resetAt));

  // For each weekday keep the in-window date with the most coverage, so the
  // two partial edge days collapse to one weekday.
  const best: ({ date: CivilDate; overlap: number } | undefined)[] = new Array(7).fill(undefined);
  let d = startDate;
  for (let i = 0; i < 10; i++) {
    const next = nextDay(d);
    const overlap = Math.min(midnight(next), resetAt) - Math.max(midnight(d), start);
    if (overlap > 0) {
      const wd = mondayIndex(d);
      const prev = best[wd];
      if (!prev || overlap > prev.overlap) best[wd] = { date: d, overlap };
    }
    if (dateKey(d) === endKey) break;
    d = next;
  }

  const dates: number[] = [];
  for (let wd = 0; wd < 7; wd++) {
    const entry = best[wd];
    if (working[wd] && entry) dates.push(dateKey(entry.date));
  }
  dates.sort((a, b) => a - b);
  const n = dates.length;
  if (n === 0) return undefined;

  const dividers: number[] = [];
  for (let k = 1; k < n; k++) dividers.push(k / n);
  const position = workingDayPosition(dates, now);
  return { dividers, current: position.current, timePct: position.timePct, workingDates: dates };
}

function workingDayPosition(
  dates: readonly number[],
  now: number,
): { current?: [number, number]; timePct: number } {
  const n = dates.length;
  const today = localDate(now);
  const k = dates.indexOf(dateKey(today));
  if (k >= 0) {
    const frac = clamp((now - midnight(today)) / 86400, 0, 1);
    return {
      current: [k / n, (k + 1) / n],
      timePct: clamp(((k + frac) / n) * 100, 0, 100),
    };
  }
  // Off-day: sit on the boundary after the last elapsed working day.
  const elapsed = dates.filter((key) => key < dateKey(today)).length;
  return { timePct: clamp((elapsed / n) * 100, 0, 100) };
}

/** Fraction (0–1) of the bar at `epoch`, on working time when reshaped. */
export function timeFraction(seg: Segments, epoch: number, resetAt: number, periodSecs: number): number {
  if (seg.workingDates.length > 0) {
    return workingDayPosition(seg.workingDates, epoch).timePct / 100;
  }
  return clamp((epoch - (resetAt - periodSecs)) / periodSecs, 0, 1);
}

/** Segments of a window (empty when it has no reset or grid). */
export function windowSegments(window: UsageWindowInfo, working: WorkingDays, now: number): Segments {
  const unit = segmentUnitForWindow(window.windowSeconds);
  if (!unit || window.resetAt === undefined) return EMPTY_SEGMENTS;
  return resetAlignedSegments(window.resetAt, window.windowSeconds, unit, working, now);
}

/** Time elapsed in a window as the desktop uses it: working-time when reshaped, else linear. */
export function effectiveTimePct(
  window: UsageWindowInfo,
  working: WorkingDays,
  now: number,
): number | undefined {
  if (window.resetAt === undefined) return undefined;
  const linear = linearTimePct(window.resetAt, window.windowSeconds, now);
  return windowSegments(window, working, now).timePct ?? linear;
}

// ── Formatting ───────────────────────────────────────────────────────────

const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];
const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/** Compact duration: `45s`, `12m`, `2h 14m`, `3d 4h`. */
export function formatDuration(seconds: number): string {
  const s = Math.max(Math.floor(seconds), 0);
  if (s < 60) return `${s}s`;
  const minutes = Math.floor(s / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ${minutes % 60}m`;
  const days = Math.floor(hours / 24);
  return `${days}d ${hours % 24}h`;
}

/** `resets in 2h 14m`; `resetting now` once the reset time passed. */
export function formatResetsIn(resetAt: number, now: number): string {
  const remaining = resetAt - now;
  if (remaining <= 0) return 'resetting now';
  if (remaining < 60) return 'resets in <1m';
  return `resets in ${formatDuration(remaining)}`;
}

/**
 * Local reset clock: `14:00`, or with `includeDate` `today 14:00` /
 * `tomorrow 09:00` / `Tue 09:00` / `Jun 21, 09:00`. Mirrors
 * `format_reset_time_epoch` without the zone name.
 */
export function formatResetClock(resetAt: number, now: number, includeDate: boolean): string {
  if (resetAt <= 0) return '';
  const date = new Date(resetAt * 1000);
  const clock = `${pad2(date.getHours())}:${pad2(date.getMinutes())}`;
  if (!includeDate) return clock;
  const today = localDate(now);
  const days = Math.round((midnight(localDate(resetAt)) - midnight(today)) / 86400);
  if (days === 0) return `today ${clock}`;
  if (days === 1) return `tomorrow ${clock}`;
  if (days >= 2 && days <= 6) return `${WEEKDAYS[date.getDay()]} ${clock}`;
  return `${MONTHS[date.getMonth()]} ${date.getDate()}, ${clock}`;
}

/** `$2.50`. */
export function formatDollars(amount: number): string {
  return `$${amount.toFixed(2)}`;
}

function capitalize(text: string): string {
  return text.length === 0 ? text : text[0]!.toUpperCase() + text.slice(1);
}

// ── Display model ────────────────────────────────────────────────────────

export type ProviderId = 'claude' | 'codex';

export const PROVIDER_NAMES: Record<ProviderId, string> = { claude: 'Claude', codex: 'Codex' };

export interface UsageWindowView {
  id: string;
  /** Row title (`Session`, `Weekly`, `Fable`, `5-hour limit`, …). */
  title: string;
  /** Period badge (`5h`, `7d`). */
  period: string;
  usedPercent: number;
  /** Time elapsed (0–100) the pace marker sits at; working-time when reshaped. */
  timePct?: number;
  /** `max(abs, pace)`: the percentage color. */
  severity: Severity;
  /** Pace alone: drives the "ahead of pace" note. */
  pace: Severity;
  segments: Segments;
  resetAt?: number;
  /** `resets in 2h 14m`. */
  resetsIn?: string;
  /** Local reset clock (`today 14:00`, `Tue 09:00`). */
  resetClock?: string;
  /** History as chart points: x on the bar's time axis (0–1), y = percent / 100. */
  history: { x: number; y: number }[];
}

export interface ProviderView {
  provider: ProviderId;
  name: string;
  status: string;
  /** Capitalized plan (`Max`, `Plus`). */
  plan?: string;
  /** Status / error line; `undefined` when there is nothing to add. */
  statusText?: string;
  /** Secondary line: data age, next attempt, rate limit. */
  detailText?: string;
  isError: boolean;
  fetching: boolean;
  windows: UsageWindowView[];
  /** Claude extra usage, when enabled. */
  extra?: { used: string; limit: string; utilization: number };
  /** Codex credit balance line (`$4.50` / `Unlimited`). */
  credits?: { text: string; unlimited: boolean };
  /** Codex reset credits summary. */
  resetCredits?: { summary: string; firstExpiry?: string; titles: string[] };
}

/** Whether a scoped Claude window gets a row (`active` or ≥ 0.5 %). */
export function shouldShowScoped(window: UsageWindowInfo): boolean {
  return window.active || window.usedPercent >= 0.5;
}

function codexWindowTitle(window: UsageWindowInfo): string {
  const period = formatWindowLabel(window.windowSeconds);
  if (period === '5h') return '5-hour limit';
  if (period === '7d') return 'Weekly limit';
  if (period === '1d') return 'Daily limit';
  return `${period} limit`;
}

/** Row title for a window, or `undefined` when the desktop hides it. */
export function windowTitle(provider: ProviderId, window: UsageWindowInfo): string | undefined {
  if (provider === 'claude') {
    switch (window.kind) {
      case 'five_hour':
        return 'Session';
      case 'weekly':
        return 'Weekly';
      case 'weekly_scoped':
        return shouldShowScoped(window) ? (window.scopedModel ?? window.label) : undefined;
      default:
        return undefined;
    }
  }
  switch (window.id) {
    case 'primary':
    case 'secondary':
      return codexWindowTitle(window);
    case 'code_review':
      return 'Code review';
    default:
      return undefined;
  }
}

export function buildWindowView(
  provider: ProviderId,
  window: UsageWindowInfo,
  working: WorkingDays,
  now: number,
): UsageWindowView | undefined {
  const title = windowTitle(provider, window);
  if (title === undefined) return undefined;
  const segments = windowSegments(window, working, now);
  const period = window.windowSeconds;
  const resetAt = window.resetAt;
  const hasTimeAxis = resetAt !== undefined && period > 0;
  const timePct = hasTimeAxis
    ? timeFraction(segments, now, resetAt, period) * 100
    : undefined;
  const history = hasTimeAxis
    ? window.history.map((p) => ({
        x: timeFraction(segments, p.recordedAt, resetAt, period),
        y: clamp(p.usedPercent / 100, 0, 1),
      }))
    : [];
  return {
    id: window.id,
    title,
    period: formatWindowLabel(period),
    usedPercent: window.usedPercent,
    timePct,
    severity: headlineSeverity(window.usedPercent, timePct),
    pace: paceSeverity(window.usedPercent, timePct),
    segments,
    resetAt,
    resetsIn: resetAt !== undefined ? formatResetsIn(resetAt, now) : undefined,
    resetClock:
      resetAt !== undefined
        ? formatResetClock(resetAt, now, segmentUnitForWindow(period) === 'day')
        : undefined,
    history,
  };
}

function isProviderId(value: string): value is ProviderId {
  return value === 'claude' || value === 'codex';
}

function statusLines(p: ProviderUsageInfo, now: number): {
  statusText?: string;
  detailText?: string;
  isError: boolean;
} {
  const details: string[] = [];
  if (p.fetchedAt !== undefined) details.push(`Updated ${formatDuration(now - p.fetchedAt)} ago`);
  if (p.rateLimitedUntil !== undefined && p.rateLimitedUntil > now) {
    details.push(`rate limited for ${formatDuration(p.rateLimitedUntil - now)}`);
  } else if (p.status === 'error' && p.nextAttemptAt !== undefined && !p.fetching) {
    details.push(`retry in ${formatDuration(p.nextAttemptAt - now)}`);
  }
  const detailText = details.length > 0 ? details.join(' · ') : undefined;
  switch (p.status) {
    case 'pending':
      return { statusText: 'Waiting for the first update…', detailText, isError: false };
    case 'no_credentials':
      return {
        statusText: p.error ?? 'Not signed in on this machine',
        detailText,
        isError: true,
      };
    case 'error':
      return {
        statusText: `Update failed${p.error ? `: ${p.error}` : ''}`,
        detailText,
        isError: true,
      };
    default:
      return { detailText, isError: false };
  }
}

function resetCreditsView(resets: ResetCreditsInfo, now: number): ProviderView['resetCredits'] {
  if (resets.availableCount === 0 && resets.credits.length === 0) return undefined;
  const count = resets.availableCount;
  const firstExpiring = resets.credits.find((c) => c.expiresAt !== undefined)?.expiresAt;
  return {
    summary: count === 1 ? '1 reset available' : `${count} resets available`,
    firstExpiry:
      firstExpiring !== undefined
        ? `first expires ${formatResetClock(firstExpiring, now, true)}`
        : undefined,
    titles: resets.credits.map((c) => c.title),
  };
}

/** One provider's view, or `undefined` when it is disabled or unknown. */
export function buildProviderView(
  p: ProviderUsageInfo,
  working: WorkingDays,
  now: number,
): ProviderView | undefined {
  if (!isProviderId(p.provider) || p.status === 'disabled') return undefined;
  const provider = p.provider;
  const windows = p.windows
    .map((w) => buildWindowView(provider, w, working, now))
    .filter((w): w is UsageWindowView => w !== undefined);
  const extra =
    p.extraUsage && p.extraUsage.isEnabled
      ? {
          used: formatDollars(p.extraUsage.usedCreditsCents / 100),
          limit: formatDollars(p.extraUsage.monthlyLimitCents / 100),
          utilization: p.extraUsage.utilization,
        }
      : undefined;
  const credits = p.credits
    ? p.credits.unlimited
      ? { text: 'Unlimited', unlimited: true }
      : p.credits.hasCredits
        ? { text: formatDollars(p.credits.balance), unlimited: false }
        : undefined
    : undefined;
  return {
    provider,
    name: PROVIDER_NAMES[provider],
    status: p.status,
    plan: p.plan ? capitalize(p.plan) : undefined,
    ...statusLines(p, now),
    fetching: p.fetching,
    windows,
    extra,
    credits,
    resetCredits: p.resetCredits ? resetCreditsView(p.resetCredits, now) : undefined,
  };
}

/** Every visible provider of a snapshot, in wire order (Claude, then Codex). */
export function buildUsageView(usage: UsageInfo, now: number): ProviderView[] {
  const working = workingDaysFromIndices(usage.workingDays);
  return usage.providers
    .map((p) => buildProviderView(p, working, now))
    .filter((p): p is ProviderView => p !== undefined);
}

// ── App bar indicator ────────────────────────────────────────────────────

export interface IndicatorItem {
  provider: ProviderId;
  name: string;
  /** The headline window's used percent; `undefined` while there is no data. */
  usedPercent?: number;
  severity: Severity;
  /** The headline window's period badge (`5h`). */
  period?: string;
}

/**
 * Windows that can headline the indicator, mirroring the desktop status bar:
 * Claude session, weekly and scoped limits that are binding or ≥ 80 %; Codex
 * primary and secondary.
 */
function headlineCandidates(provider: ProviderId, windows: UsageWindowInfo[]): UsageWindowInfo[] {
  if (provider === 'claude') {
    return windows.filter(
      (w) =>
        w.kind === 'five_hour' ||
        w.kind === 'weekly' ||
        (w.kind === 'weekly_scoped' && (w.active || w.usedPercent >= 80)),
    );
  }
  return windows.filter((w) => w.id === 'primary' || w.id === 'secondary');
}

/**
 * The most relevant window per provider for the app bar: the worst severity,
 * ties going to the first (Claude's session / Codex's primary). Providers that
 * are disabled or signed out are left out; an empty result hides the
 * indicator.
 */
export function indicatorItems(usage: UsageInfo | null | undefined, now: number): IndicatorItem[] {
  if (!usage) return [];
  const working = workingDaysFromIndices(usage.workingDays);
  const items: IndicatorItem[] = [];
  for (const p of usage.providers) {
    if (!isProviderId(p.provider)) continue;
    if (p.status === 'disabled' || p.status === 'no_credentials' || p.status === 'unknown') continue;
    let best: { window: UsageWindowInfo; severity: Severity } | undefined;
    for (const window of headlineCandidates(p.provider, p.windows)) {
      const severity = headlineSeverity(window.usedPercent, effectiveTimePct(window, working, now));
      if (!best || SEVERITY_RANK[severity] > SEVERITY_RANK[best.severity]) best = { window, severity };
    }
    items.push({
      provider: p.provider,
      name: PROVIDER_NAMES[p.provider],
      usedPercent: best?.window.usedPercent,
      severity: best?.severity ?? 'normal',
      period: best ? formatWindowLabel(best.window.windowSeconds) : undefined,
    });
  }
  return items;
}

/** Spoken summary of the indicator, e.g. `Claude 20% of 5h limit, Codex 3% …`. */
export function indicatorAccessibilityLabel(items: readonly IndicatorItem[]): string {
  const parts = items.map((item) => {
    if (item.usedPercent === undefined) return `${item.name} no data yet`;
    const pace = item.severity === 'normal' ? '' : item.severity === 'warning' ? ', warning' : ', critical';
    return `${item.name} ${Math.round(item.usedPercent)}% of ${item.period ?? ''} limit${pace}`;
  });
  return `Agent usage: ${parts.join('; ')}`;
}

/**
 * Whether a window's history is worth a sparkline: two or more samples
 * spanning at least 3 % of the bar, so a fresh daemon's first minutes don't
 * draw a sliver.
 */
export function hasHistoryShape(points: readonly { x: number }[]): boolean {
  if (points.length < 2) return false;
  return points[points.length - 1]!.x - points[0]!.x >= 0.03;
}

/** Any provider with a fetch in flight. */
export function anyFetching(usage: UsageInfo | null | undefined): boolean {
  return usage?.providers.some((p) => p.fetching) ?? false;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}
