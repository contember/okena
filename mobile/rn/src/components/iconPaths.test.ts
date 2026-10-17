import { ICONS } from './iconPaths';

describe('icon paths', () => {
  it.each(Object.entries(ICONS))('%s is an SVG path inside the 24px box', (_name, def) => {
    expect(def.d).toMatch(/^M[0-9MmLlHhVvAaZz.,\s-]+$/);
    const numbers = (def.d.match(/-?\d*\.?\d+/g) ?? []).map(Number);
    expect(numbers.length).toBeGreaterThan(0);
    for (const n of numbers) expect(Math.abs(n)).toBeLessThanOrEqual(24);
  });
});
