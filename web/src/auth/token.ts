const TOKEN_KEY = "okena_token";
const EXPIRY_KEY = "okena_token_expiry";

export function saveToken(token: string, expiresIn: number): void {
  const expiryMs = Date.now() + expiresIn * 1000;
  localStorage.setItem(TOKEN_KEY, token);
  localStorage.setItem(EXPIRY_KEY, expiryMs.toString());
}

export function loadToken(): string | null {
  const token = localStorage.getItem(TOKEN_KEY);
  const expiry = localStorage.getItem(EXPIRY_KEY);
  if (!token || !expiry) return null;

  const expiryMs = parseInt(expiry, 10);
  if (Date.now() >= expiryMs) {
    clearToken();
    return null;
  }
  return token;
}

export function clearToken(): void {
  localStorage.removeItem(TOKEN_KEY);
  localStorage.removeItem(EXPIRY_KEY);
}

/** Remaining TTL in seconds, or 0 if expired/missing */
export function tokenTtlSecs(): number {
  const expiry = localStorage.getItem(EXPIRY_KEY);
  if (!expiry) return 0;
  const remaining = (parseInt(expiry, 10) - Date.now()) / 1000;
  return Math.max(0, remaining);
}
