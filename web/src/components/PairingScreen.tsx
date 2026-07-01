import { useState, useRef, type FormEvent } from "react";
import { pair } from "../api/client";

export function PairingScreen({ onPaired }: { onPaired: () => void }) {
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    const trimmed = code.trim();
    if (!trimmed) return;

    setLoading(true);
    setError(null);

    try {
      await pair(trimmed);
      onPaired();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Pairing failed");
      inputRef.current?.focus();
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="app-shell flex h-screen items-center justify-center">
      <div className="w-[360px] border border-[var(--ok-border)] bg-[var(--ok-panel)]">
        <div className="project-header border-b border-[var(--ok-border)] px-4 py-3">
          <h1 className="text-[15px] font-bold text-[var(--ok-text)]">Okena</h1>
          <p className="mt-1 text-[11px] text-[var(--ok-text-secondary)]">
            Enter the pairing code from the desktop app status bar
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4 px-4 py-4">
          <input
            ref={inputRef}
            type="text"
            value={code}
            onChange={(e) => setCode(e.target.value.toUpperCase())}
            placeholder="XXXX-XXXX"
            maxLength={9}
            autoFocus
            className="w-full border border-[var(--ok-border)] bg-[var(--ok-terminal)] px-4 py-3 text-center font-mono text-2xl tracking-[0.2em]
              text-[var(--ok-text)] placeholder:text-[var(--ok-text-muted)]
              disabled:opacity-50"
            disabled={loading}
          />

          {error && (
            <p className="text-center text-[12px] text-[var(--ok-red)]">{error}</p>
          )}

          <button
            type="submit"
            disabled={loading || code.trim().length === 0}
            className="w-full bg-[var(--ok-blue)] py-3 font-bold text-white transition-colors
              hover:bg-[#005a9e] disabled:bg-[var(--ok-header)] disabled:text-[var(--ok-text-muted)]"
          >
            {loading ? "Pairing..." : "Connect"}
          </button>
        </form>
      </div>
    </div>
  );
}
