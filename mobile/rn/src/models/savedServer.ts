/**
 * savedServer.ts — the persisted "saved server" model.
 *
 * Ported from `mobile/lib/src/models/saved_server.dart`. Adds two fields the
 * Dart model lacks but the binding contract supports (RN ships TLS-on from day
 * one — see RN_MIGRATION.md §3 Phase 1 "TODO(mobile-tls)"):
 *   - `tls`         — whether to connect over TLS.
 *   - `fingerprint` — the pinned server certificate fingerprint (TOFU).
 *
 * The screen agents construct these (add-server sheet) and the
 * {@link import('../state/connectionStore').useConnectionStore} persists a list
 * of them.
 */

/**
 * A server the user has saved to connect to.
 *
 * Identity (equality / dedupe) is by `host` + `port` only — mirrors the Dart
 * `operator ==` / `hashCode`. See {@link savedServerEquals}.
 */
export interface SavedServer {
  /** Hostname or IP of the remote Okena desktop server. */
  readonly host: string;
  /** TCP port. */
  readonly port: number;
  /** Optional user-facing label; falls back to `host:port` for display. */
  readonly label?: string;
  /**
   * Optional saved auth token. Present once the server has been paired; lets a
   * reconnect skip the pairing step (passed to `connect` as `savedToken`).
   */
  readonly token?: string;
  /**
   * Whether to connect over TLS. Defaults to `true` for new servers (RN is
   * TLS-on by default). Not present in the Dart model.
   */
  readonly tls: boolean;
  /**
   * Pinned TLS certificate fingerprint (trust-on-first-use). Set after the
   * first successful TLS handshake; a mismatch on reconnect indicates the
   * server cert changed. Not present in the Dart model.
   */
  readonly fingerprint?: string;
}

/** The JSON shape persisted to storage (matches {@link SavedServer.toJSON}). */
export interface SavedServerJson {
  host: string;
  port: number;
  label?: string;
  token?: string;
  tls?: boolean;
  fingerprint?: string;
}

/**
 * Create a {@link SavedServer}, defaulting `tls` to `true` (RN TLS-on default).
 * Use this rather than an object literal so the default stays in one place.
 */
export function createSavedServer(params: {
  host: string;
  port: number;
  label?: string;
  token?: string;
  tls?: boolean;
  fingerprint?: string;
}): SavedServer {
  return {
    host: params.host,
    port: params.port,
    label: params.label,
    token: params.token,
    tls: params.tls ?? true,
    fingerprint: params.fingerprint,
  };
}

/**
 * Display name for a server — the `label` if set, else `host:port`.
 * Mirrors the Dart `displayName` getter.
 */
export function savedServerDisplayName(server: SavedServer): string {
  return server.label ?? `${server.host}:${server.port}`;
}

/**
 * Identity equality — by `host` + `port` only (mirrors Dart `operator ==`).
 * Used to dedupe on add and to locate the active server in the list.
 */
export function savedServerEquals(a: SavedServer, b: SavedServer): boolean {
  return a.host === b.host && a.port === b.port;
}

/**
 * Return a copy of `server` with the given fields overridden. Mirrors the Dart
 * `copyWith` (which only allowed `token`); extended here to cover `token`,
 * `fingerprint`, and `label` since those get filled in post-pairing.
 */
export function withSavedServer(
  server: SavedServer,
  patch: Partial<Pick<SavedServer, 'token' | 'fingerprint' | 'label'>>,
): SavedServer {
  return {
    ...server,
    token: patch.token ?? server.token,
    fingerprint: patch.fingerprint ?? server.fingerprint,
    label: patch.label ?? server.label,
  };
}

/**
 * Serialize a server to its JSON form. Optional fields are omitted when unset
 * (matches the Dart `toJson`, which used `if (x != null)`). `tls` is always
 * written so a round-trip is lossless.
 */
export function toJSON(server: SavedServer): SavedServerJson {
  const json: SavedServerJson = {
    host: server.host,
    port: server.port,
    tls: server.tls,
  };
  if (server.label !== undefined) json.label = server.label;
  if (server.token !== undefined) json.token = server.token;
  if (server.fingerprint !== undefined) json.fingerprint = server.fingerprint;
  return json;
}

/**
 * Parse a server from its JSON form. `tls` defaults to `true` when absent
 * (older persisted data, or Dart-written data, had no `tls` key).
 */
export function fromJSON(json: SavedServerJson): SavedServer {
  return {
    host: json.host,
    port: json.port,
    label: json.label,
    token: json.token,
    tls: json.tls ?? true,
    fingerprint: json.fingerprint,
  };
}

/**
 * Parse a JSON-array string into a list of servers (mirrors Dart
 * `listFromJson`). Throws if the string is not a JSON array; the caller
 * (connection store) catches and starts fresh on corrupted data.
 */
export function listFromJson(jsonString: string): SavedServer[] {
  const parsed: unknown = JSON.parse(jsonString);
  if (!Array.isArray(parsed)) {
    throw new TypeError('saved-server list JSON is not an array');
  }
  return parsed.map((e) => fromJSON(e as SavedServerJson));
}

/** Serialize a list of servers to a JSON-array string (mirrors Dart `listToJson`). */
export function listToJson(servers: readonly SavedServer[]): string {
  return JSON.stringify(servers.map(toJSON));
}
