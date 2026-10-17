package com.okena.terminalview

import android.content.res.AssetManager
import android.graphics.Typeface
import android.graphics.fonts.Font
import android.graphics.fonts.FontFamily
import android.graphics.fonts.FontStyle
import android.os.Build
import android.util.Log
import java.nio.ByteBuffer
import java.util.BitSet

/**
 * Typefaces and per-codepoint glyph sources for the terminal, mirroring the Skia renderer's
 * resolution order in `src/components/terminalGlyphs.ts`:
 *
 *   1. the primary font (JetBrains Mono) when its cmap has the codepoint: batched into runs;
 *   2. a primary-font stand-in from [ALIASES] (`⎿` → `└`): batched too;
 *   3. the bundled Noto Sans Symbols 2 subset, skipped for emoji-presentation codepoints so
 *      those stay colour emoji;
 *   4. the platform fallback (emoji, CJK, everything else).
 *
 * Steps 3 and 4 are drawn one cell at a time, fitted into the cell(s). On API 29+ the "chain"
 * typefaces are `Typeface.CustomFallbackBuilder(JetBrains Mono).addCustomFallback(Noto)`, which
 * then falls back to the system fonts, so graphemes with combining marks resolve per character.
 *
 * Coverage comes from parsing the fonts' cmap tables once, because `Paint.hasGlyph` also
 * counts the system fallback chain. Fonts are copied into `assets/fonts` by the app's
 * `copyTerminalFonts` Gradle task. Loaded once per process.
 */
internal class TerminalFonts private constructor(assets: AssetManager) {

  /** Plain JetBrains Mono variants, indexed by [variant]. */
  val primary: Array<Typeface>

  /** Fallback chains for per-cell glyphs, indexed by [variant]. */
  val chain: Array<Typeface>

  /** Bundled symbol font (null when it failed to load). */
  val symbols: Typeface?

  /** Typeface for emoji-presentation codepoints (system emoji font through fallback). */
  val emoji: Typeface = Typeface.DEFAULT

  private val primaryCoverage: BitSet
  private val symbolCoverage: BitSet

  init {
    primary = Array(4) { v -> load(assets, PRIMARY_FILES[v]) ?: Typeface.create(Typeface.MONOSPACE, STYLES[v]) }
    symbols = load(assets, SYMBOL_FILE)
    primaryCoverage = coverage(assets, PRIMARY_FILES[0])
    symbolCoverage = if (symbols != null) coverage(assets, SYMBOL_FILE) else BitSet()
    chain = Array(4) { v -> buildChain(assets, v) ?: primary[v] }
  }

  /** Glyph source of a single codepoint; cached per process. */
  fun resolve(cp: Int): Glyph {
    if (cp in 0x21..0x7e && primaryCoverage[cp]) return asciiGlyphs[cp]!!
    synchronized(cache) {
      cache[cp]?.let { return it }
      val g = compute(cp)
      cache[cp] = g
      return g
    }
  }

  private val asciiGlyphs = arrayOfNulls<Glyph>(0x7f).also { arr ->
    for (cp in 0x21..0x7e) arr[cp] = Glyph(KIND_PRIMARY, String(Character.toChars(cp)))
  }
  private val cache = HashMap<Int, Glyph>()

  private fun compute(cp: Int): Glyph {
    val text = String(Character.toChars(cp))
    if (primaryCoverage[cp]) return Glyph(KIND_PRIMARY, text)
    val alias = ALIASES[cp]
    if (alias != null && primaryCoverage[alias.codePointAt(0)]) return Glyph(KIND_PRIMARY, alias)
    if (!isEmojiRange(cp) && symbolCoverage[cp]) return Glyph(KIND_SYMBOL, text)
    return Glyph(KIND_SYSTEM, text)
  }

  /** `kind` is one of KIND_*; `text` is what to draw (an alias for aliased codepoints). */
  class Glyph(val kind: Int, val text: String)

  private fun buildChain(assets: AssetManager, v: Int): Typeface? {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return null
    return try {
      val main = FontFamily.Builder(Font.Builder(assets, "fonts/${PRIMARY_FILES[v]}").build()).build()
      val builder = Typeface.CustomFallbackBuilder(main)
      if (symbols != null) {
        builder.addCustomFallback(FontFamily.Builder(Font.Builder(assets, "fonts/$SYMBOL_FILE").build()).build())
      }
      val bold = v and 1 != 0
      val italic = v and 2 != 0
      builder.setStyle(
        FontStyle(
          if (bold) FontStyle.FONT_WEIGHT_BOLD else FontStyle.FONT_WEIGHT_NORMAL,
          if (italic) FontStyle.FONT_SLANT_ITALIC else FontStyle.FONT_SLANT_UPRIGHT,
        ),
      )
      builder.build()
    } catch (e: Exception) {
      Log.w(TAG, "fallback chain for variant $v failed", e)
      null
    }
  }

  companion object {
    private const val TAG = "OkenaTerminalFonts"

    const val KIND_PRIMARY = 0
    const val KIND_SYMBOL = 1
    const val KIND_SYSTEM = 2

    /** Variant index: bit 0 = bold, bit 1 = italic. */
    fun variant(bold: Boolean, italic: Boolean): Int = (if (bold) 1 else 0) or (if (italic) 2 else 0)

    private val PRIMARY_FILES = arrayOf(
      "JetBrainsMono.ttf",
      "JetBrainsMono_bold.ttf",
      "JetBrainsMono_italic.ttf",
      "JetBrainsMono_bold_italic.ttf",
    )
    private val STYLES = intArrayOf(Typeface.NORMAL, Typeface.BOLD, Typeface.ITALIC, Typeface.BOLD_ITALIC)
    private const val SYMBOL_FILE = "NotoSansSymbols2-Subset.ttf"

    /** Mirror of `GLYPH_ALIASES` in terminalGlyphs.ts. */
    val ALIASES: Map<Int, String> = mapOf(0x23bf to "└")

    /**
     * Emoji-presentation blocks, as in `isEmojiPresentation` (terminalGlyphs.ts). The caller
     * also requires the cell to be WIDE, which the frame reports per cell.
     */
    fun isEmojiRange(cp: Int): Boolean = (cp in 0x231a..0x2b55) || (cp in 0x1f000 until 0x1fb00)

    @Volatile private var instance: TerminalFonts? = null

    fun get(assets: AssetManager): TerminalFonts =
      instance ?: synchronized(this) { instance ?: TerminalFonts(assets).also { instance = it } }

    private fun load(assets: AssetManager, file: String): Typeface? =
      try {
        Typeface.createFromAsset(assets, "fonts/$file")
      } catch (e: RuntimeException) {
        Log.w(TAG, "font $file missing", e)
        null
      }

    private fun coverage(assets: AssetManager, file: String): BitSet =
      try {
        val bytes = assets.open("fonts/$file").use { it.readBytes() }
        parseCmap(ByteBuffer.wrap(bytes))
      } catch (e: Exception) {
        Log.w(TAG, "cmap of $file unreadable", e)
        // Assume ASCII so the terminal still batches text.
        BitSet().apply { set(0x20, 0x7f) }
      }

    /** Codepoints with a non-zero glyph in the font's best Unicode cmap subtable. */
    internal fun parseCmap(b: ByteBuffer): BitSet {
      val out = BitSet()
      val numTables = b.getShort(4).toInt() and 0xffff
      var cmap = -1
      for (i in 0 until numTables) {
        val rec = 12 + i * 16
        if (b.getInt(rec) == 0x636d6170) { // 'cmap'
          cmap = b.getInt(rec + 8)
          break
        }
      }
      if (cmap < 0) return out
      val n = b.getShort(cmap + 2).toInt() and 0xffff
      var best12 = -1
      var best4 = -1
      for (i in 0 until n) {
        val rec = cmap + 4 + i * 8
        val platform = b.getShort(rec).toInt() and 0xffff
        val encoding = b.getShort(rec + 2).toInt() and 0xffff
        val sub = cmap + b.getInt(rec + 4)
        val format = b.getShort(sub).toInt() and 0xffff
        val unicode = platform == 0 || (platform == 3 && (encoding == 1 || encoding == 10))
        if (!unicode) continue
        if (format == 12 && best12 < 0) best12 = sub
        if (format == 4 && best4 < 0) best4 = sub
      }
      if (best12 >= 0) {
        val groups = b.getInt(best12 + 12)
        for (g in 0 until groups) {
          val p = best12 + 16 + g * 12
          val start = b.getInt(p)
          val end = minOf(b.getInt(p + 4), 0x10ffff)
          val startGlyph = b.getInt(p + 8)
          if (start > end || start < 0) continue
          if (startGlyph != 0) out.set(start, end + 1) else if (end > start) out.set(start + 1, end + 1)
        }
        return out
      }
      if (best4 >= 0) {
        val segX2 = b.getShort(best4 + 6).toInt() and 0xffff
        val ends = best4 + 14
        val starts = ends + segX2 + 2
        val deltas = starts + segX2
        val ranges = deltas + segX2
        for (s in 0 until segX2 / 2) {
          val end = b.getShort(ends + s * 2).toInt() and 0xffff
          val start = b.getShort(starts + s * 2).toInt() and 0xffff
          val delta = b.getShort(deltas + s * 2).toInt()
          val rangeOff = b.getShort(ranges + s * 2).toInt() and 0xffff
          for (c in start..end) {
            if (c == 0xffff) continue
            val glyph = if (rangeOff == 0) {
              (c + delta) and 0xffff
            } else {
              val addr = ranges + s * 2 + rangeOff + 2 * (c - start)
              val g = b.getShort(addr).toInt() and 0xffff
              if (g == 0) 0 else (g + delta) and 0xffff
            }
            if (glyph != 0) out.set(c)
          }
        }
      }
      return out
    }
  }
}
