package com.okena.terminalview

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Rect
import android.graphics.RenderNode
import android.os.Build
import android.os.Trace
import android.util.Log
import android.view.Choreographer
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.VelocityTracker
import android.view.View
import android.view.ViewConfiguration
import android.view.accessibility.AccessibilityManager
import android.widget.EdgeEffect
import android.widget.OverScroller
import androidx.core.view.ViewCompat
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.floor
import kotlin.math.roundToInt

/**
 * The native terminal renderer (Android). It replaces the Skia view (`TerminalView.tsx`) and
 * its JS gestures (`TerminalPane.tsx`) when the `renderer` setting is `native`.
 *
 * Data comes straight from the Rust core through [RustBridge] (the C ABI in
 * `crates/okena-mobile-ffi/include/okena_render.h`):
 *
 *   - **Frame loop.** A [Choreographer] callback polls the lock-free `generation` once per
 *     vsync while the view is attached and visible. Only a new generation, a scroll or a size
 *     change leads to a snapshot and a redraw; otherwise a frame costs one JNI call.
 *   - **Rows.** Each visible row is a [RenderNode] on API 29+, re-recorded only when its
 *     (absolute line, hash) changes; scrolling replays the cached nodes at a new offset. API
 *     24–28 draw rows directly. [RowPainter] holds the drawing rules.
 *   - **Scroll.** [ScrollModel] (absolute lines, sub-line pixel offset), [OverScroller] fling,
 *     [EdgeEffect] at both ends. In mouse mode (`wheel` returns true) a drag sends discrete
 *     wheel notches and the view does not move.
 *   - **Overlays.** Selection, cursor and the scroll indicator are drawn over the rows.
 *   - **Gestures.** Tap → [Listener.onTap]; double tap selects and copies a word; long press
 *     starts a selection, drag extends it (auto-scrolls at the edges), release copies it;
 *     pinch scales live and reports the snapped size on release.
 *   - **Sizing.** cols/rows from the glyph advance and line height (as `measureCell` in
 *     TerminalView.tsx), `resizeLocal` at once, `resizeTerminal` at once for the first size
 *     and debounced by 200 ms after that.
 *
 * [OkenaTerminalViewManager] copies props onto this view and turns [Listener] callbacks into
 * React Native events. Keep this class free of RN code.
 *
 * Units: JS works in dp. [fontSize] and every size passed to [Listener] are in dp; drawing
 * and measurement use px internally.
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
      reopen()
    }

  var terminalId: String = ""
    set(value) {
      if (field == value) return
      field = value
      reopen()
    }

  /** Font size in dp. */
  var fontSize: Double = 10.5
    set(value) {
      if (field == value || !(value > 0)) return
      field = value
      applyFontSize()
    }

  /** Theme colors as packed ARGB (`0xAARRGGBB`). Cell colors are resolved by the Rust core. */
  var backgroundArgb: Int = 0xff000000.toInt()
    set(value) {
      field = value
      invalidate()
    }
  var foregroundArgb: Int = 0xffcdd6f4.toInt()
  var cursorArgb: Int = 0xfff5e0dc.toInt()
    set(value) {
      field = value
      invalidate()
    }
  var selectionArgb: Int = 0x40264f78
    set(value) {
      if (field == value) return
      field = value
      discardRows() // the tint is baked into the cached rows
      invalidate()
    }

  /** Whether this pane is the keyboard / toolbar target. */
  var focusedPane: Boolean = false

  // ── Commands ─────────────────────────────────────────────────────────────

  fun scrollToBottom() {
    // JS calls this after input. Input makes this client the daemon's resize owner ("last to
    // interact wins"), so a size that was denied before can be applied now.
    if (sentCols > 0 && (frame.cols != sentCols || frame.screenLines != sentRows)) {
      reassertCount = 0
      scheduleReassert()
    }
    scroller.forceFinished(true)
    flingWheel = false
    model.toBottom()
    contentDirty = true
    invalidate()
    emitScrollState()
  }

  fun clearSelection() {
    if (handle != 0L) RustBridge.selectionClear(handle)
    selecting = false
    autoScroll = 0f
  }

  // ── State ────────────────────────────────────────────────────────────────

  private val density = resources.displayMetrics.density
  private val fonts = TerminalFonts.get(context.assets)
  private val painter = RowPainter(fonts)
  private val frame = RvFrame()
  private val model = ScrollModel()

  private var handle = 0L
  private var attached = false
  private var windowVisible = true

  private var metrics: CellMetrics? = null
  private var cols = 0
  private var rows = 0
  private var sentCols = 0
  private var sentRows = 0
  private var initialResizeSent = false
  private var terminalPresent = false

  private var lastGeneration = -1L
  private var contentDirty = true
  private var snapTop = Long.MIN_VALUE
  private var snapRows = -1
  private var reportedAtBottom = true

  // Per-row cache (API 29+): absolute line → recorded row. `node` is a RenderNode.
  /** `sel` is the row's selected column span baked into the node (`selectionKey`). */
  private class CachedRow(val node: Any, var hash: Long, var sel: Long, var version: Int)
  private val rowCache = HashMap<Long, CachedRow>()
  private var layoutVersion = 0
  private val useRenderNodes = Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q

  private var rowTexts: Array<String?> = emptyArray()

  private val overlayPaint = Paint().apply { isAntiAlias = false }
  private val backgroundPaint = Paint().apply { isAntiAlias = false }
  private val indicatorPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = 0x40ffffff }
  private val cursorStroke = Paint().apply {
    isAntiAlias = false
    style = Paint.Style.STROKE
  }

  // ── Frame loop ───────────────────────────────────────────────────────────

  private val choreographer = Choreographer.getInstance()
  private var framePosted = false
  private val frameCallback = Choreographer.FrameCallback { onVsync() }

  private fun schedule() {
    if (framePosted || !attached || !windowVisible || handle == 0L) return
    framePosted = true
    choreographer.postFrameCallback(frameCallback)
  }

  private fun onVsync() {
    framePosted = false
    if (handle == 0L || !attached) return
    var redraw = false
    val gen = RustBridge.generation(handle)
    if (gen != lastGeneration) {
      lastGeneration = gen
      contentDirty = true
      redraw = true
    }
    if (stepFling()) redraw = true
    if (stepAutoScroll()) redraw = true
    if (!topEdge.isFinished || !bottomEdge.isFinished) redraw = true
    if (redraw) invalidate()
    schedule()
  }

  // ── Lifecycle ────────────────────────────────────────────────────────────

  override fun onAttachedToWindow() {
    super.onAttachedToWindow()
    attached = true
    reopen()
  }

  override fun onDetachedFromWindow() {
    attached = false
    closeHandle()
    super.onDetachedFromWindow()
  }

  override fun onWindowVisibilityChanged(visibility: Int) {
    super.onWindowVisibilityChanged(visibility)
    windowVisible = visibility == VISIBLE
    if (windowVisible) {
      contentDirty = true
      invalidate()
      schedule()
    }
  }

  private fun reopen() {
    closeHandle()
    if (!attached || connId.isEmpty() || terminalId.isEmpty()) {
      invalidate()
      return
    }
    handle = try {
      RustBridge.open(connId, terminalId)
    } catch (e: Throwable) {
      Log.e(TAG, "open failed", e)
      0L
    }
    initialResizeSent = false
    sentCols = 0
    sentRows = 0
    terminalPresent = false
    if (handle != 0L) sendSize()
    contentDirty = true
    invalidate()
    schedule()
  }

  private fun closeHandle() {
    removeCallbacks(resizeRunnable)
    removeCallbacks(reassertRunnable)
    removeCallbacks(clearWordRunnable)
    scroller.forceFinished(true)
    flingWheel = false
    selecting = false
    autoScroll = 0f
    if (framePosted) {
      choreographer.removeFrameCallback(frameCallback)
      framePosted = false
    }
    if (handle != 0L) {
      RustBridge.close(handle)
      handle = 0L
    }
    frame.load(null)
    model.reset()
    discardRows()
    rowTexts = emptyArray()
    lastGeneration = -1L
    snapTop = Long.MIN_VALUE
    snapRows = -1
    contentDirty = true
    emitScrollState()
  }

  // ── Measurement / sizing ─────────────────────────────────────────────────

  private fun applyFontSize() {
    metrics = null
    discardRows()
    updateGrid()
    invalidate()
  }

  /** Mirrors `measureCell` in TerminalView.tsx: advance of "M", (ascent+descent) × 1.2. */
  private fun ensureMetrics(): CellMetrics {
    metrics?.let { return it }
    painter.setTextSize((fontSize * density).toFloat())
    val advance = painter.measureAdvance()
    val fm = painter.fontMetrics()
    val textHeight = -fm.ascent + fm.descent
    val cellHeight = textHeight * LINE_HEIGHT_FACTOR
    val baseline = (cellHeight - textHeight) / 2f - fm.ascent
    val m = CellMetrics(advance, cellHeight, baseline, RowPainter.stroke(fontSize.toFloat(), density), density)
    metrics = m
    return m
  }

  override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
    super.onSizeChanged(w, h, oldw, oldh)
    topEdge.setSize(w, h)
    bottomEdge.setSize(w, h)
    updateGrid()
  }

  private fun updateGrid() {
    if (width <= 0 || height <= 0) return
    val m = ensureMetrics()
    if (m.cellWidth <= 0f || m.cellHeight <= 0f) return
    val newCols = floor(width / m.cellWidth).toInt().coerceIn(1, MAX_COLS)
    val newRows = floor(height / m.cellHeight).toInt().coerceIn(1, MAX_ROWS)
    if (newCols != cols || newRows != rows) {
      cols = newCols
      rows = newRows
      layoutVersion++
      sendSize()
    }
    contentDirty = true
    invalidate()
    // Report even when cols/rows are unchanged: a font change still moves the cell size.
    listener?.onGridSize(cols, rows, (m.cellWidth / density).toDouble(), (m.cellHeight / density).toDouble())
  }

  /** resize_local now; resize_terminal now the first time, then debounced by 200 ms. */
  private fun sendSize() {
    if (handle == 0L || cols <= 0 || rows <= 0) return
    if (cols == sentCols && rows == sentRows) return
    sentCols = cols
    sentRows = rows
    reassertCount = 0
    RustBridge.resizeLocal(handle, cols, rows)
    removeCallbacks(resizeRunnable)
    if (!initialResizeSent) {
      initialResizeSent = true
      RustBridge.resizeTerminal(handle, cols, rows)
    } else {
      postDelayed(resizeRunnable, RESIZE_DEBOUNCE_MS)
    }
  }

  /**
   * The server can resize the local grid after our resize (its terminal create / resize
   * messages carry the PTY size it had). The view owns sizing, so re-send ours when the frame
   * disagrees, a few times per size so two clients of one terminal cannot fight forever.
   */
  private var reassertCount = 0

  private fun scheduleReassert() {
    if (reassertCount >= MAX_REASSERTS) return
    removeCallbacks(reassertRunnable)
    postDelayed(reassertRunnable, REASSERT_DELAY_MS)
  }

  private val reassertRunnable = Runnable {
    if (handle == 0L || sentCols <= 0) return@Runnable
    if (frame.cols == sentCols && frame.screenLines == sentRows) return@Runnable
    reassertCount++
    RustBridge.resizeLocal(handle, sentCols, sentRows)
    RustBridge.resizeTerminal(handle, sentCols, sentRows)
  }

  private val resizeRunnable = Runnable {
    if (handle != 0L && sentCols > 0) RustBridge.resizeTerminal(handle, sentCols, sentRows)
  }

  // ── Snapshot ─────────────────────────────────────────────────────────────

  /** Rows to read: everything that can be visible, including a partial row at the bottom. */
  private fun windowRows(m: CellMetrics): Int =
    (ceil(height / m.cellHeight).toInt() + 1).coerceIn(1, MAX_SNAPSHOT_ROWS)

  private fun snapshot(top: Long, count: Int) {
    frame.load(RustBridge.snapshot(handle, top, count))
    snapTop = top
    snapRows = count
  }

  /** Make the frame match the scroll position and the latest output. */
  private fun ensureFrame(m: CellMetrics) {
    val count = windowRows(m)
    var top = floor(model.top).toLong()
    if (!contentDirty && top == snapTop && count == snapRows && frame.buf != null) return
    contentDirty = false
    snapshot(top, count)
    val present = frame.status == RvFrame.STATUS_OK
    if (present != terminalPresent) {
      terminalPresent = present
      if (present) {
        // The terminal appeared after open: size it now (earlier resizes were no-ops).
        initialResizeSent = false
        sentCols = 0
        sentRows = 0
        sendSize()
      } else {
        discardRows()
      }
    }
    if (!present) return
    if ((frame.screenLines != sentRows || frame.cols != sentCols) && sentCols > 0) scheduleReassert()
    model.update(frame.firstLine, frame.screenTopLine, frame.screenLines, rows, frame.reflowEpoch)
    val wanted = floor(model.top).toLong()
    if (wanted != top) {
      top = wanted
      snapshot(top, count)
    }
    emitScrollState()
    if (accessibilityOn()) {
      rowTexts = Array(frame.rows) { r ->
        if (frame.rowFlags(r) and RvFrame.ROW_VOID != 0) null else painter.rowText(frame, r)
      }
      a11y.invalidateRoot()
    }
  }

  // ── Drawing ──────────────────────────────────────────────────────────────

  override fun onDraw(canvas: Canvas) {
    super.onDraw(canvas)
    // RN parents do not clip children, and drawColor fills the whole clip: stay in bounds.
    canvas.clipRect(0, 0, width, height)
    backgroundPaint.color = backgroundArgb
    canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), backgroundPaint)
    if (handle == 0L || width <= 0 || height <= 0) return
    Trace.beginSection("OkenaTerminal.draw")
    try {
      drawTerminal(canvas)
    } finally {
      Trace.endSection()
    }
  }

  private fun drawTerminal(canvas: Canvas) {
    val m = ensureMetrics()
    ensureFrame(m)
    if (frame.status != RvFrame.STATUS_OK || frame.rows == 0) return

    val top = snapTop
    val sub = ((model.top - top) * m.cellHeight).toFloat()
    val pinching = pinchScale != 1f
    if (pinching) {
      canvas.save()
      canvas.scale(pinchScale, pinchScale, pinchFocusX, pinchFocusY)
    }

    drawRows(canvas, m, sub)
    drawCursor(canvas, m, top, sub)

    if (pinching) canvas.restore()
    drawIndicator(canvas)
    drawEdges(canvas)
  }

  private fun drawRows(canvas: Canvas, m: CellMetrics, sub: Float) {
    // The grid's cells in the terminal's default bg, the margins in the theme bg: the Skia
    // view painted every cell (Rust default bg != theme bg) and left the remainder alone.
    var firstRow = -1
    var lastRow = -1
    for (r in 0 until frame.rows) {
      if (frame.rowFlags(r) and RvFrame.ROW_VOID != 0) continue
      if (firstRow < 0) firstRow = r
      lastRow = r
    }
    if (firstRow >= 0) {
      backgroundPaint.color = frame.defaultBg
      canvas.drawRect(
        0f,
        firstRow * m.cellHeight - sub,
        frame.cols * m.cellWidth,
        (lastRow + 1) * m.cellHeight - sub,
        backgroundPaint,
      )
    }
    val hw = useRenderNodes && canvas.isHardwareAccelerated
    val rowHeight = ceil(m.cellHeight).toInt() + 1
    for (r in 0 until frame.rows) {
      if (frame.rowFlags(r) and RvFrame.ROW_VOID != 0) continue
      val y = r * m.cellHeight - sub
      if (y >= height || y + m.cellHeight <= 0f) continue
      canvas.save()
      canvas.translate(0f, y)
      if (hw) {
        drawCachedRow(canvas, r, m, rowHeight)
      } else {
        painter.drawRow(canvas, frame, r, m, selectionKey(frame.rowLine(r)), selectionArgb)
      }
      canvas.restore()
    }
    if (hw && rowCache.size > frame.rows * 3) evictRows(frame.rows)
  }

  private fun drawCachedRow(canvas: Canvas, r: Int, m: CellMetrics, rowHeight: Int) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
    val line = frame.rowLine(r)
    val hash = frame.rowHash(r)
    val sel = selectionKey(line)
    var entry = rowCache[line]
    if (entry == null || entry.hash != hash || entry.sel != sel || entry.version != layoutVersion) {
      val node = (entry?.node as RenderNode?) ?: RenderNode("okena-row").apply { setClipToBounds(false) }
      node.setPosition(0, 0, width, rowHeight)
      val rc = node.beginRecording(width, rowHeight)
      try {
        painter.drawRow(rc, frame, r, m, sel, selectionArgb)
      } finally {
        node.endRecording()
      }
      if (entry == null) {
        entry = CachedRow(node, hash, sel, layoutVersion)
        rowCache[line] = entry
      } else {
        entry.hash = hash
        entry.sel = sel
        entry.version = layoutVersion
      }
    }
    canvas.drawRenderNode(entry.node as RenderNode)
  }

  /** Drop cached rows far from the current window (keeps a window above and below). */
  private fun evictRows(window: Int) {
    val lo = snapTop - window
    val hi = snapTop + window * 2
    val it = rowCache.entries.iterator()
    while (it.hasNext()) {
      val e = it.next()
      if (e.key < lo || e.key > hi) {
        discardNode(e.value.node)
        it.remove()
      }
    }
  }

  private fun discardRows() {
    for (row in rowCache.values) discardNode(row.node)
    rowCache.clear()
    layoutVersion++
  }

  private fun discardNode(node: Any) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) (node as RenderNode).discardDisplayList()
  }

  /**
   * The selected columns of absolute `line`, packed as `first shl 32 or last` (inclusive), or
   * [RowPainter.NO_SELECTION]. The selection is baked into the row under the text, as the Skia
   * painter drew it, so the span is part of a cached row's key.
   */
  private fun selectionKey(line: Long): Long {
    val kind = frame.selKind
    if (kind == RvFrame.SEL_NONE || line < frame.selStartLine || line > frame.selEndLine) {
      return RowPainter.NO_SELECTION
    }
    val c0: Int
    val c1: Int
    if (kind == RvFrame.SEL_BLOCK) {
      c0 = minOf(frame.selStartCol, frame.selEndCol)
      c1 = maxOf(frame.selStartCol, frame.selEndCol)
    } else {
      c0 = if (line == frame.selStartLine) frame.selStartCol else 0
      c1 = if (line == frame.selEndLine) frame.selEndCol else frame.cols - 1
    }
    val last = minOf(c1, frame.cols - 1)
    return if (c0 <= last) (c0.toLong() shl 32) or last.toLong() else RowPainter.NO_SELECTION
  }

  private fun drawCursor(canvas: Canvas, m: CellMetrics, top: Long, sub: Float) {
    if (!frame.cursorVisible) return
    val r = frame.cursorLine - top
    if (r < 0 || r >= frame.rows || frame.cursorCol >= frame.cols) return
    val x = frame.cursorCol * m.cellWidth
    val y = r * m.cellHeight - sub
    val cw = m.cellWidth
    val ch = m.cellHeight
    val half = density // a 2 dp line, centered like Skia's stroked line
    when (frame.cursorShape) {
      RvFrame.CURSOR_BEAM -> {
        overlayPaint.color = cursorArgb
        canvas.drawRect(x - half, y, x + half, y + ch, overlayPaint)
      }
      RvFrame.CURSOR_UNDERLINE -> {
        overlayPaint.color = cursorArgb
        val uy = y + ch - density
        canvas.drawRect(x, uy - half, x + cw, uy + half, overlayPaint)
      }
      RvFrame.CURSOR_HOLLOW_BLOCK -> {
        cursorStroke.color = cursorArgb
        cursorStroke.strokeWidth = density
        val inset = density / 2
        canvas.drawRect(x + inset, y + inset, x + cw - inset, y + ch - inset, cursorStroke)
      }
      else -> {
        // Half-alpha block, as withAlpha(128) in the Skia painter.
        overlayPaint.color = (0x80 shl 24) or (cursorArgb and 0x00ffffff)
        canvas.drawRect(x, y, x + cw, y + ch, overlayPaint)
      }
    }
  }

  /** The Skia scrollback thumb: 3 dp wide, 4 dp from the right edge, at least 20 dp tall. */
  private fun drawIndicator(canvas: Canvas) {
    val visible = frame.screenLines
    val total = frame.historySize + visible
    if (visible <= 0 || total <= visible) return
    val track = height.toFloat()
    val thumb = (visible.toFloat() / total * track).coerceIn(minOf(20f * density, track), track)
    val maxOffset = (model.maxTop - model.minTop).toDouble()
    val offset = model.distanceFromBottom.coerceAtMost(maxOffset)
    val thumbTop = if (maxOffset > 0) ((1 - offset / maxOffset) * (track - thumb)).toFloat() else 0f
    val left = width - 4f * density
    val r = 1.5f * density
    canvas.drawRoundRect(left, thumbTop, left + 3f * density, thumbTop + thumb, r, r, indicatorPaint)
  }

  // ── Scrolling ────────────────────────────────────────────────────────────

  private val viewConfig = ViewConfiguration.get(context)
  private val touchSlop = viewConfig.scaledTouchSlop
  private val minFling = viewConfig.scaledMinimumFlingVelocity
  private val maxFling = viewConfig.scaledMaximumFlingVelocity
  private val scroller = OverScroller(context)
  private val topEdge = EdgeEffect(context)
  private val bottomEdge = EdgeEffect(context)
  private var velocity: VelocityTracker? = null

  /** Fling origin (absolute line at scroller y = 0) and end (px) for a view fling. */
  private var flingOrigin = 0L
  private var flingMaxY = 0
  /** A fling that turns into wheel notches (mouse mode) instead of moving the view. */
  private var flingWheel = false
  private var flingLastY = 0
  private var wheelAccum = 0f

  private fun mouseMode(): Boolean {
    val mode = frame.mode
    return mode and RvFrame.MODE_MOUSE != 0 ||
      (mode and RvFrame.MODE_ALT_SCREEN != 0 && mode and RvFrame.MODE_ALTERNATE_SCROLL != 0)
  }

  /** Move by a finger delta (px, positive = finger down = older output). */
  private fun dragBy(dy: Float, x: Float, y: Float, wheelMode: Boolean) {
    val m = metrics ?: return
    if (wheelMode) {
      wheelBy(dy, x, y, m)
      return
    }
    val over = model.scrollBy((-dy / m.cellHeight).toDouble())
    if (over < 0) {
      topEdge.onPull(abs(dy) / height, x / width)
      if (!bottomEdge.isFinished) bottomEdge.onRelease()
    } else if (over > 0) {
      bottomEdge.onPull(abs(dy) / height, 1f - x / width)
      if (!topEdge.isFinished) topEdge.onRelease()
    }
    invalidate()
    emitScrollState()
  }

  /** Accumulate px into whole notches for `wheel`; scroll the view if the app did not take them. */
  private fun wheelBy(dy: Float, x: Float, y: Float, m: CellMetrics) {
    wheelAccum += dy
    val notches = (wheelAccum / m.cellHeight).toInt()
    if (notches == 0) return
    wheelAccum -= notches * m.cellHeight
    val (col, row) = screenCell(x, y, m)
    if (!RustBridge.wheel(handle, notches, col, row)) {
      model.scrollBy(-notches.toDouble())
      invalidate()
      emitScrollState()
    }
  }

  private fun startFling(vy: Float, wheelMode: Boolean) {
    val m = metrics ?: return
    scroller.forceFinished(true)
    if (wheelMode) {
      flingWheel = true
      flingLastY = 0
      scroller.fling(0, 0, 0, vy.toInt(), 0, 0, -WHEEL_FLING_RANGE, WHEEL_FLING_RANGE)
    } else {
      flingWheel = false
      flingOrigin = model.minTop
      val y0 = ((model.top - flingOrigin) * m.cellHeight).roundToInt()
      flingMaxY = ((model.maxTop - flingOrigin) * m.cellHeight).roundToInt()
      scroller.fling(0, y0, 0, (-vy).toInt(), 0, 0, 0, flingMaxY)
    }
    schedule()
  }

  /** Advance a running fling; returns true when it moved anything. */
  private fun stepFling(): Boolean {
    if (scroller.isFinished) return false
    val m = metrics ?: return false
    if (!scroller.computeScrollOffset()) return false
    val y = scroller.currY
    if (flingWheel) {
      val d = (y - flingLastY).toFloat()
      flingLastY = y
      wheelBy(d, width / 2f, height / 2f, m)
      return true
    }
    model.scrollTo(flingOrigin + y / m.cellHeight.toDouble())
    // OverScroller stops exactly at a bound; hand the remaining speed to the edge effect.
    val v = scroller.currVelocity.toInt()
    if (v > 0 && (y <= 0 || y >= flingMaxY)) {
      if (y <= 0) topEdge.onAbsorb(v) else bottomEdge.onAbsorb(v)
      scroller.forceFinished(true)
    }
    emitScrollState()
    return true
  }

  private fun drawEdges(canvas: Canvas) {
    if (!topEdge.isFinished) {
      if (topEdge.draw(canvas)) postInvalidateOnAnimation()
    }
    if (!bottomEdge.isFinished) {
      canvas.save()
      canvas.translate(width.toFloat(), height.toFloat())
      canvas.rotate(180f)
      if (bottomEdge.draw(canvas)) postInvalidateOnAnimation()
      canvas.restore()
    }
  }

  private fun emitScrollState() {
    val atBottom = model.follow
    if (atBottom == reportedAtBottom) return
    reportedAtBottom = atBottom
    listener?.onScrollStateChange(atBottom)
  }

  // ── Selection ────────────────────────────────────────────────────────────

  private var selecting = false
  /** Lines per frame while a selection drag sits at an edge (negative = up). */
  private var autoScroll = 0f
  private var lastX = 0f
  private var lastY = 0f

  private class Hit(val line: Long, val col: Int, val side: Int)

  /** Absolute line, buffer column and cell half under a point. */
  private fun hit(x: Float, y: Float, m: CellMetrics): Hit {
    val line = floor(model.top + y / m.cellHeight).toLong()
    val maxCol = (frame.cols - 1).coerceAtLeast(0)
    val fx = x / m.cellWidth
    val col = floor(fx).toInt().coerceIn(0, maxCol)
    val side = if (fx - floor(fx) < 0.5f) RvFrame.SIDE_LEFT else RvFrame.SIDE_RIGHT
    return Hit(line, col, side)
  }

  /** Cell of the live screen under a point (row 0 = top screen row, as `wheel` expects). */
  private fun screenCell(x: Float, y: Float, m: CellMetrics): Pair<Int, Int> {
    val maxCol = ((if (frame.cols > 0) frame.cols else cols) - 1).coerceAtLeast(0)
    val maxRow = ((if (frame.screenLines > 0) frame.screenLines else rows) - 1).coerceAtLeast(0)
    val col = floor(x / m.cellWidth).toInt().coerceIn(0, maxCol)
    val line = floor(model.top + y / m.cellHeight).toLong()
    val row = (line - frame.screenTopLine).coerceIn(0L, maxRow.toLong()).toInt()
    return col to row
  }

  private fun startSelection(x: Float, y: Float) {
    val m = metrics ?: return
    if (handle == 0L || frame.status != RvFrame.STATUS_OK) return
    removeCallbacks(clearWordRunnable)
    val h = hit(x, y, m)
    RustBridge.selectionStart(handle, h.line, h.col, h.side, RvFrame.SELECT_SIMPLE)
    selecting = true
    lastX = x
    lastY = y
    parent?.requestDisallowInterceptTouchEvent(true)
    performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
    invalidate()
  }

  private fun extendSelection(x: Float, y: Float) {
    val m = metrics ?: return
    lastX = x
    lastY = y
    val edge = m.cellHeight
    autoScroll = when {
      y < edge -> -((edge - y) / edge).coerceIn(0.1f, 1f) * AUTO_SCROLL_MAX
      y > height - edge -> ((y - (height - edge)) / edge).coerceIn(0.1f, 1f) * AUTO_SCROLL_MAX
      else -> 0f
    }
    val h = hit(x, y.coerceIn(0f, height - 1f), m)
    RustBridge.selectionUpdate(handle, h.line, h.col, h.side)
    schedule()
  }

  private fun stepAutoScroll(): Boolean {
    if (!selecting || autoScroll == 0f) return false
    model.scrollBy(autoScroll.toDouble())
    val m = metrics ?: return true
    val h = hit(lastX, lastY.coerceIn(0f, height - 1f), m)
    RustBridge.selectionUpdate(handle, h.line, h.col, h.side)
    emitScrollState()
    return true
  }

  /** Copy the selection to the clipboard, report it, then clear it (as the Skia pane does). */
  private fun copySelection(clearAfterMs: Long = 0L) {
    if (handle == 0L) return
    val text = RustBridge.selectionText(handle)
    if (!text.isNullOrEmpty()) {
      val clipboard = context.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager?
      clipboard?.setPrimaryClip(ClipData.newPlainText("terminal", text))
      listener?.onSelectionCopied(text)
    }
    if (clearAfterMs > 0) {
      postDelayed(clearWordRunnable, clearAfterMs)
    } else {
      RustBridge.selectionClear(handle)
    }
  }

  private val clearWordRunnable = Runnable { if (handle != 0L) RustBridge.selectionClear(handle) }

  private fun finishSelection() {
    selecting = false
    autoScroll = 0f
    copySelection()
  }

  // ── Pinch ────────────────────────────────────────────────────────────────

  private var pinchScale = 1f
  private var pinchStartSize = 0.0
  private var pinchFocusX = 0f
  private var pinchFocusY = 0f
  /** Set once a pinch happened in the current touch sequence: no taps or drags after it. */
  private var pinchedThisGesture = false

  private val scaleDetector = ScaleGestureDetector(
    context,
    object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
      override fun onScaleBegin(detector: ScaleGestureDetector): Boolean {
        if (selecting) return false
        pinchedThisGesture = true
        dragging = false
        scroller.forceFinished(true)
        pinchStartSize = fontSize
        pinchScale = 1f
        pinchFocusX = detector.focusX
        pinchFocusY = detector.focusY
        parent?.requestDisallowInterceptTouchEvent(true)
        return true
      }

      override fun onScale(detector: ScaleGestureDetector): Boolean {
        val next = pinchScale * detector.scaleFactor
        val size = (pinchStartSize * next).coerceIn(MIN_FONT_SIZE, MAX_FONT_SIZE)
        pinchScale = (size / pinchStartSize).toFloat()
        pinchFocusX = detector.focusX
        pinchFocusY = detector.focusY
        invalidate()
        return true
      }

      override fun onScaleEnd(detector: ScaleGestureDetector) {
        val raw = pinchStartSize * pinchScale
        val snapped = ((raw / FONT_STEP).roundToInt() * FONT_STEP).coerceIn(MIN_FONT_SIZE, MAX_FONT_SIZE)
        pinchScale = 1f
        if (snapped != fontSize) {
          fontSize = snapped
          listener?.onFontSizeChange(snapped)
        } else {
          invalidate()
        }
      }
    },
  )

  // ── Taps / long press ────────────────────────────────────────────────────

  /** The touch that stopped a fling is not a tap. */
  private var downStoppedFling = false

  private val gestures = GestureDetector(
    context,
    object : GestureDetector.SimpleOnGestureListener() {
      override fun onDown(e: MotionEvent): Boolean = true

      override fun onSingleTapUp(e: MotionEvent): Boolean {
        if (downStoppedFling || pinchedThisGesture) return false
        val m = metrics ?: return false
        if (frame.selKind != RvFrame.SEL_NONE && handle != 0L) {
          RustBridge.selectionClear(handle)
          return true
        }
        val (col, row) = screenCell(e.x, e.y, m)
        listener?.onTap(col, row)
        return true
      }

      override fun onDoubleTap(e: MotionEvent): Boolean {
        if (pinchedThisGesture) return false
        val m = metrics ?: return false
        if (handle == 0L || frame.status != RvFrame.STATUS_OK) return false
        val h = hit(e.x, e.y, m)
        removeCallbacks(clearWordRunnable)
        RustBridge.selectWord(handle, h.line, h.col)
        // Keep the word highlighted briefly so the copy is visible.
        copySelection(clearAfterMs = WORD_FLASH_MS)
        return true
      }

      override fun onLongPress(e: MotionEvent) {
        if (dragging || pinchedThisGesture || scaleDetector.isInProgress) return
        startSelection(e.x, e.y)
      }
    },
  )

  // ── Touch ────────────────────────────────────────────────────────────────

  private var dragging = false
  private var dragWheel = false
  private var downY = 0f
  private var dragLastY = 0f

  override fun onTouchEvent(event: MotionEvent): Boolean {
    if (handle == 0L) return super.onTouchEvent(event)
    val action = event.actionMasked
    if (action == MotionEvent.ACTION_DOWN) {
      pinchedThisGesture = false
      downStoppedFling = !scroller.isFinished
      scroller.forceFinished(true)
      flingWheel = false
      velocity?.recycle()
      velocity = VelocityTracker.obtain()
    }
    velocity?.addMovement(event)
    scaleDetector.onTouchEvent(event)
    if (!pinchedThisGesture || action == MotionEvent.ACTION_UP || action == MotionEvent.ACTION_CANCEL) {
      gestures.onTouchEvent(event)
    }

    when (action) {
      MotionEvent.ACTION_DOWN -> {
        downY = event.y
        dragLastY = event.y
        dragging = false
        wheelAccum = 0f
      }
      MotionEvent.ACTION_MOVE -> {
        if (selecting) {
          extendSelection(event.x, event.y)
        } else if (!pinchedThisGesture && event.pointerCount == 1) {
          val y = event.y
          if (!dragging && abs(y - downY) > touchSlop) {
            dragging = true
            dragWheel = mouseMode()
            dragLastY = y
            parent?.requestDisallowInterceptTouchEvent(true)
          }
          if (dragging) {
            dragBy(y - dragLastY, event.x, y, dragWheel)
            dragLastY = y
          }
        }
      }
      MotionEvent.ACTION_UP -> {
        if (selecting) {
          finishSelection()
        } else if (dragging && !pinchedThisGesture) {
          val vt = velocity
          vt?.computeCurrentVelocity(1000, maxFling.toFloat())
          val vy = vt?.yVelocity ?: 0f
          if (abs(vy) > minFling) startFling(vy, dragWheel)
        }
        endTouch()
        performClick()
      }
      MotionEvent.ACTION_CANCEL -> {
        if (selecting) finishSelection()
        endTouch()
      }
    }
    return true
  }

  private fun endTouch() {
    dragging = false
    velocity?.recycle()
    velocity = null
    topEdge.onRelease()
    bottomEdge.onRelease()
    if (!topEdge.isFinished || !bottomEdge.isFinished) postInvalidateOnAnimation()
  }

  override fun performClick(): Boolean = super.performClick()

  // ── Accessibility (TalkBack: one virtual node per visible row) ───────────

  private val accessibilityManager =
    context.getSystemService(Context.ACCESSIBILITY_SERVICE) as AccessibilityManager?

  private fun accessibilityOn(): Boolean = accessibilityManager?.isTouchExplorationEnabled == true

  private fun subPx(m: CellMetrics): Float = ((model.top - snapTop) * m.cellHeight).toFloat()

  private val a11y = TerminalAccessibility(
    this,
    rowCount = { rowTexts.size },
    rowText = { r -> rowTexts.getOrNull(r) },
    rowBounds = { r ->
      metrics?.let { m ->
        val y = r * m.cellHeight - subPx(m)
        Rect(0, y.toInt(), width, (y + m.cellHeight).toInt())
      }
    },
    rowAt = { y -> metrics?.let { m -> floor((y + subPx(m)) / m.cellHeight).toInt() } ?: -1 },
  )

  init {
    ViewCompat.setAccessibilityDelegate(this, a11y)
    importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_YES
    isHapticFeedbackEnabled = true
  }

  override fun dispatchHoverEvent(event: MotionEvent): Boolean =
    a11y.dispatchHoverEvent(event) || super.dispatchHoverEvent(event)

  private companion object {
    const val TAG = "OkenaTerminalView"
    const val LINE_HEIGHT_FACTOR = 1.2f
    const val MAX_COLS = 500
    const val MAX_ROWS = 200
    const val MAX_SNAPSHOT_ROWS = 1024
    const val RESIZE_DEBOUNCE_MS = 200L
    const val REASSERT_DELAY_MS = 300L
    const val MAX_REASSERTS = 2
    const val WORD_FLASH_MS = 250L
    const val MIN_FONT_SIZE = 8.0
    const val MAX_FONT_SIZE = 20.0
    const val FONT_STEP = 0.5
    /** Max auto-scroll speed while a selection drag sits at an edge, lines per frame. */
    const val AUTO_SCROLL_MAX = 0.6f
    const val WHEEL_FLING_RANGE = 1_000_000
  }
}
