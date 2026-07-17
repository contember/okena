import { createSavedServer } from '../src/models/savedServer';
import type { OkenaNative } from '../src/native/okena';
import { connectSavedServer } from '../src/state/connectionStore';

describe('connectSavedServer', () => {
  it('passes the TLS setting and certificate pin to the native API', () => {
    const connect = jest.fn(() => 'conn-1');
    const native: Pick<OkenaNative, 'connect'> = { connect };
    const server = createSavedServer({
      host: 'okena.lan',
      port: 19100,
      token: 'saved-token',
      tls: true,
      fingerprint: 'sha256:certificate-pin',
    });

    expect(connectSavedServer(native, server)).toBe('conn-1');
    expect(connect).toHaveBeenCalledWith(
      'okena.lan',
      19100,
      'saved-token',
      true,
      'sha256:certificate-pin',
    );
  });

  it('preserves an explicit plaintext connection without a pin', () => {
    const connect = jest.fn(() => 'conn-2');
    const native: Pick<OkenaNative, 'connect'> = { connect };
    const server = createSavedServer({
      host: '127.0.0.1',
      port: 19100,
      tls: false,
    });

    expect(connectSavedServer(native, server)).toBe('conn-2');
    expect(connect).toHaveBeenCalledWith(
      '127.0.0.1',
      19100,
      undefined,
      false,
      undefined,
    );
  });
});
