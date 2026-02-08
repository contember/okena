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
    <div className="flex items-center justify-center h-screen">
      <div className="w-80 space-y-6">
        <div className="text-center">
          <h1 className="text-2xl font-bold text-zinc-100">Okena</h1>
          <p className="mt-2 text-sm text-zinc-400">
            Enter the pairing code from the desktop app status bar
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <input
            ref={inputRef}
            type="text"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            placeholder="000000"
            maxLength={6}
            autoFocus
            className="w-full px-4 py-3 text-center text-2xl tracking-[0.3em] font-mono
              bg-zinc-900 border border-zinc-700 rounded-lg text-zinc-100
              placeholder:text-zinc-600 focus:outline-none focus:border-blue-500
              disabled:opacity-50"
            disabled={loading}
          />

          {error && (
            <p className="text-sm text-red-400 text-center">{error}</p>
          )}

          <button
            type="submit"
            disabled={loading || code.trim().length === 0}
            className="w-full py-3 bg-blue-600 hover:bg-blue-500 disabled:bg-zinc-700
              disabled:text-zinc-500 text-white font-medium rounded-lg
              transition-colors"
          >
            {loading ? "Pairing..." : "Connect"}
          </button>
        </form>
      </div>
    </div>
  );
}
