package com.okena.terminalview

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.view.GestureDetector
import android.view.MotionEvent
import android.view.View
import com.facebook.react.common.assets.ReactFontManager
import kotlin.math.floor

/**
 * The native terminal renderer (Android).
 *
 * WU2 state: a placeholder. It paints the theme background with a small label,
 * measures a monospace cell from the terminal font, reports the grid size from
 * layout and turns a single tap into a cell. It does not call Rust yet; WU5/WU6
 * replace the placeholder with the Canvas row renderer, the scroll model and the
 * gestures (see docs/sprints/sprint-2026-10-17-native-terminal-view.md).
 *
 * [OkenaTerminalViewManager] copies props onto this view and turns [Listener]
 * callbacks into React Native events. Keep this class free of RN event code.
 *
 * Units: JS works in dp (as the Skia renderer does). [fontSize] and every size
 * passed to [Listener] are in dp; drawing and measurement use px internally.
 */
class TerminalRendererView(context: Context) : View(context) {

  /** Callbacks forwarded to JS as events. Sizes are in dp. */
  interface Listener {
    fun onTap(col: Int, row: Int)
    fun onGridSize(cols: Int, rows: Int, cellWidth: Double, cellHeight: Double)
    fun onFontSizeChange(size: Double)
    fun onSelectionCopied(text: String)
    fun onScrollStateChange(atBottom: Boolean)
  }

  var listener: Listener? = null

  // ── Props ────────────────────────────────────────────────────────────────

  var connId: String = ""
    set(value) {
      if (field == value) return
      field = value
      invalidate()
    }

  var terminalId: String = ""
    set(value) {
      if (field == value) return
      field = value
      invalidate()
    }

  /** Font size in dp. */
  var fontSize: Double = 10.5
    set(value) {
      if (field == value || !(value > 0)) return
      field = value
      updateGrid()
    }

  /** Theme colors as packed ARGB (`0xAARRGGBB`), as resolved by the Rust core. */
  var backgroundArgb: Int = 0xff000000.toInt()
    set(value) {
      field = value
      invalidate()
    }
  var foregroundArgb: Int = 0xffcdd6f4.toInt()
    set(value) {
      field = value
      invalidate()
    }
  var cursorArgb: Int = 0xfff5e0dc.toInt()
  var selectionArgb: Int = 0x40264f78

  /** Whether this pane is the keyboard / toolbar target. */
  var focusedPane: Boolean = false

  // ── Commands ─────────────────────────────────────────────────────────────

  fun scrollToBottom() {
    // Placeholder: always at the bottom. WU6 resets the scroll offset here.
    listener?.onScrollStateChange(true)
  }

  fun clearSelection() {
    // Placeholder: no selection yet (WU6).
  }

  // ── Measurement / layout ───────────────────────────────────────────────────

  private data class Grid(val cols: Int, val rows: Int, val cellWidthPx: Float, val cellHeightPx: Float)

  private val density = resources.displayMetrics.density
  private val cellPaint = Paint(Paint.ANTI_ALIAS_FLAG)
  private val labelPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply {
    typeface = Typeface.MONOSPACE
    textSize = 11f * resources.displayMetrics.density
  }
  private var grid: Grid? = null

  private fun terminalTypeface(): Typeface =
    try {
      // The app copies JetBrainsMono-*.ttf into assets/fonts (see app/build.gradle).
      ReactFontManager.getInstance().getTypeface("JetBrainsMono", Typeface.NORMAL, context.assets)
    } catch (_: RuntimeException) {
      Typeface.MONOSPACE
    }

  /**
   * Mirrors `measureCell` in TerminalView.tsx: the advance of "M", and the
   * ascent + descent times the line-height factor. Returns px.
   */
  private fun measureCell(): Pair<Float, Float> {
    cellPaint.typeface = terminalTypeface()
    cellPaint.textSize = (fontSize * density).toFloat()
    val advance = cellPaint.measureText("M")
    val metrics = cellPaint.fontMetrics
    val textHeight = -metrics.ascent + metrics.descent
    return advance to textHeight * LINE_HEIGHT_FACTOR
  }

  override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
    super.onSizeChanged(w, h, oldw, oldh)
    updateGrid()
  }

  private fun updateGrid() {
    if (width <= 0 || height <= 0) return
    val (cellW, cellH) = measureCell()
    if (cellW <= 0f || cellH <= 0f) return
    val cols = floor(width / cellW).toInt().coerceIn(1, MAX_COLS)
    val rows = floor(height / cellH).toInt().coerceIn(1, MAX_ROWS)
    val next = Grid(cols, rows, cellW, cellH)
    if (next == grid) return
    grid = next
    invalidate()
    // WU5: resize_local here, then a 200 ms debounced resize_terminal.
    listener?.onGridSize(cols, rows, (cellW / density).toDouble(), (cellH / density).toDouble())
  }

  // ── Drawing ──────────────────────────────────────────────────────────────

  override fun onDraw(canvas: Canvas) {
    super.onDraw(canvas)
    canvas.drawColor(backgroundArgb)
    labelPaint.color = (foregroundArgb and 0x00ffffff) or (0x99 shl 24)
    val pad = 12f * density
    val lineHeight = labelPaint.fontSpacing
    val lines = buildList {
      add("Native terminal (placeholder)")
      add("$connId / $terminalId")
      grid?.let { add("${it.cols}×${it.rows}") }
    }
    lines.forEachIndexed { i, line ->
      canvas.drawText(line, pad, pad + lineHeight * (i + 1), labelPaint)
    }
  }

  // ── Gestures ─────────────────────────────────────────────────────────────

  private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
    override fun onDown(e: MotionEvent): Boolean = true

    override fun onSingleTapUp(e: MotionEvent): Boolean {
      val g = grid ?: return false
      val col = floor(e.x / g.cellWidthPx).toInt().coerceIn(0, g.cols - 1)
      val row = floor(e.y / g.cellHeightPx).toInt().coerceIn(0, g.rows - 1)
      listener?.onTap(col, row)
      return true
    }
  })

  override fun onTouchEvent(event: MotionEvent): Boolean {
    val handled = gestures.onTouchEvent(event)
    if (event.actionMasked == MotionEvent.ACTION_UP) performClick()
    return handled || super.onTouchEvent(event)
  }

  override fun performClick(): Boolean = super.performClick()

  private companion object {
    const val LINE_HEIGHT_FACTOR = 1.2f
    const val MAX_COLS = 500
    const val MAX_ROWS = 200
  }
}
