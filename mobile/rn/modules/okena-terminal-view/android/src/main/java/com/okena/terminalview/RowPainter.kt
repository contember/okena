package com.okena.terminalview

import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import kotlin.math.max
import kotlin.math.roundToInt

/** Cell geometry in px, from the primary font at the current size. */
internal data class CellMetrics(
  val cellWidth: Float,
  val cellHeight: Float,
  /** Baseline offset from the top of a row. */
  val baseline: Float,
  /** Decoration stroke width (underline, strike). */
  val stroke: Float,
  val density: Float,
)

/**
 * Draws one terminal row (backgrounds, selection, glyphs, underline, strike) from a frame into a canvas
 * whose origin is the row's top-left corner. The rules mirror `paintTerminal` in
 * `src/components/TerminalView.tsx`:
 *
 *   - a background rect wherever the cell bg differs from the default bg (no anti-aliasing);
 *   - the selection tint over the backgrounds and under the text;
 *   - glyphs batched into runs of the same fg and style; dim halves the fg alpha;
 *   - glyphs outside the primary font drawn one cell at a time, shrunk to fit their cell (two
 *     for WIDE cells) and centered;
 *   - underline 1 dp above the row bottom, strike through the middle, both
 *     `max(1, fontSize / 14)` dp thick.
 *
 * Differences on purpose: underline and strike cover spaces that carry them (as on desktop and
 * in other terminals; Skia skipped blank cells), and JetBrains Mono ligatures are disabled
 * because Skia never shaped text. The iOS painter follows the same rules.
 *
 * Inverse and hidden are already resolved in Rust (fg/bg swapped, hidden fg == bg).
 */
internal class RowPainter(private val fonts: TerminalFonts) {

  private val textPaints = Array(4) { v -> textPaint(fonts.primary[v]) }
  private val chainPaints = Array(4) { v -> textPaint(fonts.chain[v]) }
  private val symbolPaint = textPaint(fonts.symbols ?: fonts.chain[0])
  private val emojiPaint = textPaint(fonts.emoji)
  private val bgPaint = Paint().apply {
    isAntiAlias = false
    style = Paint.Style.FILL
  }
  private val linePaint = Paint().apply {
    isAntiAlias = false
    style = Paint.Style.FILL
  }

  /** Measured advances of per-cell glyphs, per paint, for the current size. */
  private val advances = HashMap<Paint, HashMap<String, Float>>()
  private var chars = CharArray(256)

  fun setTextSize(px: Float) {
    for (p in textPaints) p.textSize = px
    for (p in chainPaints) p.textSize = px
    symbolPaint.textSize = px
    emojiPaint.textSize = px
    advances.clear()
  }

  /** Advance of "M" (the cell width) with the regular primary font at the current size. */
  fun measureAdvance(): Float = textPaints[0].measureText("M")

  fun fontMetrics(): Paint.FontMetrics = textPaints[0].fontMetrics

  /** `selection` is the row's selected span from `TerminalRendererView.selectionKey`. */
  fun drawRow(canvas: Canvas, f: RvFrame, r: Int, m: CellMetrics, selection: Long, selectionArgb: Int) {
    val cols = f.cols
    if (cols <= 0) return
    val cw = m.cellWidth
    val ch = m.cellHeight
    val defaultBg = f.defaultBg

    // ── Pass 1: backgrounds, merged into spans of equal colour ──────────────
    var c = 0
    while (c < cols) {
      val bg = f.bg(f.cellAt(r, c))
      if (bg == defaultBg || (bg ushr 24) == 0) {
        c++
        continue
      }
      val start = c
      c++
      while (c < cols && f.bg(f.cellAt(r, c)) == bg) c++
      bgPaint.color = bg
      canvas.drawRect(start * cw, 0f, c * cw, ch, bgPaint)
    }
    if (selection != NO_SELECTION) {
      val s0 = (selection ushr 32).toInt()
      val s1 = (selection and 0xffffffffL).toInt()
      bgPaint.color = selectionArgb
      canvas.drawRect(s0 * cw, 0f, (s1 + 1) * cw, ch, bgPaint)
    }

    // ── Pass 2: glyphs ──────────────────────────────────────────────────────
    if (chars.size < cols * 2 + 4) chars = CharArray(cols * 2 + 4)
    c = 0
    while (c < cols) {
      val cell = f.cellAt(r, c)
      val flags = f.flags(cell)
      if (flags and SKIP_FLAGS != 0) {
        c++
        continue
      }
      val cp = f.codepoint(cell)
      val extra = f.extra(cell)
      if (extra == 0 && (cp == 0x20 || cp == 0)) {
        c++
        continue
      }
      val fg = f.fg(cell)
      val style = flags and STYLE_MASK
      val wide = flags and RvFrame.CELL_WIDE != 0
      val glyph = if (extra == 0) fonts.resolve(cp) else null

      if (glyph == null || wide || glyph.kind != TerminalFonts.KIND_PRIMARY) {
        // One cell (two for WIDE) from a fallback typeface, fitted and centered.
        val span = if (wide && c + 1 < cols) 2 else 1
        val text = if (glyph != null) glyph.text else (f.extraText(extra) ?: String(Character.toChars(cp)))
        val paint = when {
          glyph != null && glyph.kind == TerminalFonts.KIND_SYMBOL -> symbolPaint
          wide && TerminalFonts.isEmojiRange(cp) -> emojiPaint
          glyph != null && glyph.kind == TerminalFonts.KIND_PRIMARY -> textPaints[variant(style)]
          else -> chainPaints[variant(style)]
        }
        paint.color = effectiveFg(fg, style)
        drawFitted(canvas, paint, text, c * cw, span * cw, m)
        c += span
        continue
      }

      // A run of primary-font cells with the same fg and style.
      val start = c
      var len = appendText(glyph.text, 0)
      var trimmed = len
      c++
      while (c < cols) {
        val ci = f.cellAt(r, c)
        val fl = f.flags(ci)
        if (fl and (SKIP_FLAGS or RvFrame.CELL_WIDE) != 0) break
        if (f.extra(ci) != 0) break
        if (f.fg(ci) != fg || (fl and STYLE_MASK) != style) break
        val cpi = f.codepoint(ci)
        if (cpi == 0x20 || cpi == 0) {
          chars[len++] = ' '
        } else {
          val g = fonts.resolve(cpi)
          if (g.kind != TerminalFonts.KIND_PRIMARY) break
          len = appendText(g.text, len)
          trimmed = len
        }
        c++
      }
      val paint = textPaints[variant(style)]
      paint.color = effectiveFg(fg, style)
      canvas.drawText(chars, 0, trimmed, start * cw, m.baseline, paint)
    }

    // ── Pass 3: underline / strike, merged over cells of the same colour and style ──
    c = 0
    var runStart = -1
    var runEnd = 0
    var runColor = 0
    var runDeco = 0
    while (c < cols) {
      val cell = f.cellAt(r, c)
      val flags = f.flags(cell)
      val deco = flags and DECORATIONS
      if (deco == 0 || flags and SKIP_FLAGS != 0) {
        if (runStart >= 0) decorate(canvas, runDeco, runColor, runStart * cw, (runEnd - runStart) * cw, m)
        runStart = -1
        c++
        continue
      }
      val color = effectiveFg(f.fg(cell), flags)
      if (runStart >= 0 && (color != runColor || deco != runDeco)) {
        decorate(canvas, runDeco, runColor, runStart * cw, (runEnd - runStart) * cw, m)
        runStart = -1
      }
      if (runStart < 0) {
        runStart = c
        runColor = color
        runDeco = deco
      }
      val span = if (flags and RvFrame.CELL_WIDE != 0 && c + 1 < cols) 2 else 1
      runEnd = c + span
      c += span
    }
    if (runStart >= 0) decorate(canvas, runDeco, runColor, runStart * cw, (runEnd - runStart) * cw, m)
  }

  /** The row's text for accessibility: graphemes in column order, spacers skipped. */
  fun rowText(f: RvFrame, r: Int): String {
    val sb = StringBuilder(f.cols)
    for (c in 0 until f.cols) {
      val cell = f.cellAt(r, c)
      if (f.flags(cell) and (RvFrame.CELL_WIDE_SPACER or RvFrame.CELL_LEADING_WIDE_SPACER) != 0) continue
      val extra = f.extra(cell)
      val text = if (extra != 0) f.extraText(extra) else null
      if (text != null) {
        sb.append(text)
      } else {
        val cp = f.codepoint(cell)
        if (cp == 0) sb.append(' ') else sb.appendCodePoint(cp)
      }
    }
    return sb.trimEnd().toString()
  }

  private fun appendText(text: String, at: Int): Int {
    var i = at
    for (ch in text) chars[i++] = ch
    return i
  }

  private fun drawFitted(canvas: Canvas, paint: Paint, text: String, x: Float, target: Float, m: CellMetrics) {
    val cache = advances.getOrPut(paint) { HashMap() }
    val advance = cache.getOrPut(text) { paint.measureText(text) }
    if (advance <= 0f) return
    val scale = if (advance > target) target / advance else 1f
    val dx = (target - advance * scale) / 2f
    if (scale == 1f) {
      canvas.drawText(text, x + dx, m.baseline, paint)
      return
    }
    val centerY = m.cellHeight / 2f
    canvas.save()
    canvas.translate(x + dx, centerY)
    canvas.scale(scale, scale)
    canvas.drawText(text, 0f, m.baseline - centerY, paint)
    canvas.restore()
  }

  private fun decorate(canvas: Canvas, style: Int, color: Int, x: Float, w: Float, m: CellMetrics) {
    if (style and (UNDERLINES or RvFrame.CELL_STRIKE) == 0) return
    linePaint.color = color
    val s = m.stroke
    if (style and UNDERLINES != 0) {
      // Skia: a line of width `s` centered 1 dp above the row bottom.
      val uy = m.cellHeight - m.density
      canvas.drawRect(x, uy - s / 2f, x + w, uy + s / 2f, linePaint)
      if (style and RvFrame.CELL_DOUBLE_UNDERLINE != 0) {
        val uy2 = uy - s * 2f
        canvas.drawRect(x, uy2 - s / 2f, x + w, uy2 + s / 2f, linePaint)
      }
    }
    if (style and RvFrame.CELL_STRIKE != 0) {
      val sy = m.cellHeight / 2f
      canvas.drawRect(x, sy - s / 2f, x + w, sy + s / 2f, linePaint)
    }
  }

  private fun variant(style: Int): Int =
    TerminalFonts.variant(style and RvFrame.CELL_BOLD != 0, style and RvFrame.CELL_ITALIC != 0)

  companion object {
    private const val SKIP_FLAGS =
      RvFrame.CELL_WIDE_SPACER or RvFrame.CELL_LEADING_WIDE_SPACER or RvFrame.CELL_HIDDEN
    private const val UNDERLINES =
      RvFrame.CELL_UNDERLINE or RvFrame.CELL_DOUBLE_UNDERLINE or RvFrame.CELL_UNDERCURL or
        RvFrame.CELL_DOTTED_UNDERLINE or RvFrame.CELL_DASHED_UNDERLINE
    private const val DECORATIONS = UNDERLINES or RvFrame.CELL_STRIKE
    const val NO_SELECTION = -1L
    private const val STYLE_MASK =
      RvFrame.CELL_BOLD or RvFrame.CELL_ITALIC or RvFrame.CELL_STRIKE or RvFrame.CELL_DIM or UNDERLINES

    /** Dim halves the alpha, as in the Skia painter. */
    fun effectiveFg(fg: Int, style: Int): Int {
      if (style and RvFrame.CELL_DIM == 0) return fg
      val a = ((fg ushr 24) * 0.5f).roundToInt()
      return (a shl 24) or (fg and 0x00ffffff)
    }

    private fun textPaint(tf: Typeface): Paint =
      Paint(Paint.ANTI_ALIAS_FLAG or Paint.SUBPIXEL_TEXT_FLAG).apply {
        typeface = tf
        // Skia drew unshaped glyphs: no programming ligatures.
        fontFeatureSettings = "'calt' 0, 'liga' 0"
      }

    fun stroke(fontSizeDp: Float, density: Float): Float = max(1f, fontSizeDp / 14f) * density
  }
}
