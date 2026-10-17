import {
  GlyphFallback,
  fitGlyph,
  isEmojiPresentation,
  isWideCodepoint,
  type GlyphFallbackDeps,
} from './terminalGlyphs';

const cp = (ch: string) => ch.codePointAt(0)!;

describe('codepoint classification', () => {
  it('treats ASCII, box drawing and text symbols as narrow', () => {
    for (const ch of ['a', '─', '█', '⏺', '✻', '✔', '⚠', '⎿', '❯', '⠋']) {
      expect(isWideCodepoint(cp(ch))).toBe(false);
    }
  });

  it('treats CJK, fullwidth forms and emoji as wide', () => {
    for (const ch of ['中', '한', '＋', 'あ', '✅', '⚡', '✨', '❌', '🤖', '🔍']) {
      expect(isWideCodepoint(cp(ch))).toBe(true);
    }
  });

  it('marks only wide emoji-block symbols as emoji presentation', () => {
    expect(isEmojiPresentation(cp('✅'))).toBe(true);
    expect(isEmojiPresentation(cp('🤖'))).toBe(true);
    expect(isEmojiPresentation(cp('⏺'))).toBe(false);
    expect(isEmojiPresentation(cp('✔'))).toBe(false);
    expect(isEmojiPresentation(cp('中'))).toBe(false);
  });
});

describe('fitGlyph', () => {
  it('centers glyphs narrower than the box', () => {
    expect(fitGlyph(6, 10)).toEqual({ scale: 1, dx: 2 });
  });

  it('shrinks glyphs wider than the box', () => {
    expect(fitGlyph(20, 10)).toEqual({ scale: 0.5, dx: 0 });
  });

  it('leaves degenerate input alone', () => {
    expect(fitGlyph(0, 10)).toEqual({ scale: 1, dx: 0 });
  });
});

interface FakeFont {
  size: number;
  getGlyphWidths(glyphs: number[]): number[];
}

function makeDeps(opts: {
  primary: string;
  bundled?: string | null;
  system?: string;
}) {
  const calls = { primary: 0, bundled: 0, system: [] as [string, boolean][], fonts: 0 };
  const deps: GlyphFallbackDeps<string, FakeFont> = {
    primaryGlyphId: (ch) => {
      calls.primary++;
      return opts.primary.includes(ch) ? 1 : 0;
    },
    bundled:
      opts.bundled == null
        ? null
        : {
            typeface: 'symbols',
            glyphId: (ch) => {
              calls.bundled++;
              return opts.bundled!.includes(ch) ? 7 : 0;
            },
          },
    system: (ch, emoji) => {
      calls.system.push([ch, emoji]);
      return (opts.system ?? '').includes(ch) ? { typeface: 'system', glyph: 9 } : null;
    },
    makeFont: (_typeface, size) => {
      calls.fonts++;
      return { size, getGlyphWidths: (g) => g.map(() => size) };
    },
  };
  return { deps, calls };
}

describe('GlyphFallback', () => {
  it('uses the primary font when it has the glyph, and caches the answer', () => {
    const { deps, calls } = makeDeps({ primary: 'a✓' });
    const fb = new GlyphFallback(deps);
    expect(fb.resolve(cp('✓'))).toEqual({ kind: 'primary', text: '✓' });
    expect(fb.resolve(cp('✓'))).toBe(fb.resolve(cp('✓')));
    expect(calls.primary).toBe(1);
  });

  it('draws aliased symbols with the primary font', () => {
    const { deps, calls } = makeDeps({ primary: '└', bundled: '⎿', system: '⎿' });
    const fb = new GlyphFallback(deps);
    expect(fb.resolve(cp('⎿'))).toEqual({ kind: 'primary', text: '└' });
    expect(calls.bundled).toBe(0);
  });

  it('prefers the bundled symbol font for text symbols', () => {
    const { deps, calls } = makeDeps({ primary: '', bundled: '⏺', system: '⏺' });
    const fb = new GlyphFallback(deps);
    expect(fb.resolve(cp('⏺'))).toMatchObject({
      kind: 'fallback',
      typeface: 'symbols',
      glyph: 7,
      wide: false,
    });
    expect(calls.system).toEqual([]);
  });

  it('sends emoji to the system fallback even if the bundled font has them', () => {
    const { deps, calls } = makeDeps({ primary: '', bundled: '✅', system: '✅' });
    const fb = new GlyphFallback(deps);
    expect(fb.resolve(cp('✅'))).toMatchObject({ kind: 'fallback', typeface: 'system', wide: true });
    expect(calls.bundled).toBe(0);
    expect(calls.system).toEqual([['✅', true]]);
  });

  it('falls back to the system for glyphs outside the bundled font', () => {
    const { deps, calls } = makeDeps({ primary: '', bundled: '', system: '中' });
    const fb = new GlyphFallback(deps);
    expect(fb.resolve(cp('中'))).toMatchObject({ kind: 'fallback', typeface: 'system', wide: true });
    expect(calls.system).toEqual([['中', false]]);
  });

  it('works before the bundled font has loaded', () => {
    const { deps } = makeDeps({ primary: '', bundled: null, system: '✻' });
    const fb = new GlyphFallback(deps);
    expect(fb.resolve(cp('✻'))).toMatchObject({ kind: 'fallback', typeface: 'system' });
  });

  it('lets the primary font draw codepoints nothing covers', () => {
    const { deps } = makeDeps({ primary: '', bundled: '', system: '' });
    const fb = new GlyphFallback(deps);
    expect(fb.resolve(0xe0b0)).toEqual({ kind: 'primary', text: '' });
  });

  it('caches sized fonts per size', () => {
    const { deps, calls } = makeDeps({ primary: '', bundled: '⏺✻' });
    const fb = new GlyphFallback(deps);
    const dot = fb.resolve(cp('⏺'));
    const star = fb.resolve(cp('✻'));
    if (dot.kind !== 'fallback' || star.kind !== 'fallback') throw new Error('expected fallback');

    const a = fb.sized(dot, 14);
    expect(a).toMatchObject({ advance: 14, glyphs: [7] });
    expect(fb.sized(dot, 14)).toBe(a);
    fb.sized(star, 14);
    expect(calls.fonts).toBe(2);

    expect(fb.sized(dot, 18).advance).toBe(18);
    expect(calls.fonts).toBe(3);
  });
});
