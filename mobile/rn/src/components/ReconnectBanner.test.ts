/// <reference types="jest" />

import { reconnectLabel } from './ReconnectBanner';

describe('reconnectLabel', () => {
  it('shows attempt and elapsed time while reconnecting', () => {
    expect(reconnectLabel({ kind: 'reconnecting', attempt: 2 }, 7)).toBe(
      'Reconnecting… · attempt 2 · 7s',
    );
  });

  it('omits the attempt for a restarted connect and a zero clock', () => {
    expect(reconnectLabel({ kind: 'connecting' }, 0)).toBe('Reconnecting…');
  });
});
