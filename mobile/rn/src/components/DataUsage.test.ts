/// <reference types="jest" />

import { formatBytes, trafficRows } from './DataUsage';

describe('data usage', () => {
  it('formats byte counts', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(234 * 1024)).toBe('234 KB');
    expect(formatBytes(12.3 * 1024 * 1024)).toBe('12 MB');
    expect(formatBytes(3 * 1024 ** 3)).toBe('3.0 GB');
  });

  it('lists each kind and a total', () => {
    const rows = trafficRows({
      ptyBytes: 1024,
      snapshotBytes: 1024,
      stateBytes: 2048,
      stateFetches: 1,
      settingsBytes: 512,
      eventBytes: 512,
    });
    expect(rows.map((r) => r.value)).toEqual([
      '1.0 KB',
      '1.0 KB',
      '2.0 KB · 1 fetch',
      '1.0 KB',
      '5.0 KB',
    ]);
    expect(rows.at(-1)?.label).toBe('Total');
  });
});
