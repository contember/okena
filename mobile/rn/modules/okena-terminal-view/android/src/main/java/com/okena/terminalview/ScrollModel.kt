package com.okena.terminalview

/**
 * The view's scroll position in the line-addressing model of `okena_render.h`.
 *
 * [top] is the absolute line at the view's top edge, fractional for sub-line pixel scrolling.
 * The readable range is `[minTop, maxTop]` with `minTop = first_line` and `maxTop` the top
 * edge that shows the grid's bottom row (`screen_top_line` when the grid fits the view).
 *
 *   - At the live tail ([follow]) the view sits at [liveTop] and tracks it as output streams.
 *     [liveTop] equals `maxTop` unless the grid has more rows than the view (another client
 *     owns the PTY size, so ours was denied; or the soft keyboard just shrank the view). Then
 *     the live tail is the window that shows the cursor, preferring the screen's top: a short
 *     shell prompt sits at the top of a tall, mostly blank grid, and showing the grid's bottom
 *     rows would show nothing. The rows below stay reachable by scrolling down to `maxTop`.
 *   - Scrolled up, [top] stays on the same absolute line, so the text does not move while
 *     history grows; it is clamped when the oldest lines are dropped (first_line grows).
 *   - When `reflow_epoch` changes (any resize) the distance from the bottom is kept, because
 *     reflow can move lines.
 */
internal class ScrollModel {
  var top = 0.0
    private set
  var follow = true
    private set
  var minTop = 0L
    private set
  var maxTop = 0L
    private set
  /** The live tail's top edge, in `[minTop, maxTop]`. */
  var liveTop = 0L
    private set
  /**
   * Set while a selection gesture runs: the view keeps its absolute line instead of tracking
   * the live tail, so the text under the finger does not move.
   */
  var hold = false
  private var epoch = 0
  private var known = false

  /** Lines between the view's top and the live tail's top. */
  val distanceFromBottom: Double get() = (maxTop - top).coerceAtLeast(0.0)

  fun reset() {
    known = false
    follow = true
    top = 0.0
  }

  /**
   * Apply the bounds of a new frame. Returns true when [top] changed. [viewRows] is how many
   * rows the view shows and [cursorLine] the cursor's absolute line: when the grid is taller
   * than the view, the live tail is the window that shows the cursor (see the class doc).
   */
  fun update(
    firstLine: Long,
    screenTopLine: Long,
    screenLines: Int,
    viewRows: Int,
    reflowEpoch: Int,
    cursorLine: Long,
  ): Boolean {
    val extra = maxOf(0, screenLines - viewRows)
    val bottom = screenTopLine + extra
    val cursorRow = (cursorLine - screenTopLine).coerceIn(0L, maxOf(0, screenLines - 1).toLong())
    val live = screenTopLine + (cursorRow - viewRows + 1).coerceIn(0L, extra.toLong())
    val before = top
    val liveMoved = live != liveTop
    if (!known) {
      known = true
      follow = true
    } else if (reflowEpoch != epoch && !follow) {
      top = bottom - (maxTop - top)
    }
    val reflowed = reflowEpoch != epoch
    epoch = reflowEpoch
    minTop = minOf(firstLine, live)
    maxTop = bottom
    liveTop = live
    // Following, the view sits on the live tail; it may also rest below it (between the tail
    // and the grid's bottom) until the tail moves or the grid reflows.
    if (follow && !hold && (liveMoved || reflowed || top < live)) top = live.toDouble()
    clamp()
    return top != before
  }

  /**
   * Move by [lines] (positive = down, towards the tail). Returns the part that did not fit:
   * negative past the oldest line, positive past the tail.
   */
  fun scrollBy(lines: Double): Double {
    val wanted = top + lines
    top = wanted
    clamp()
    return wanted - top
  }

  fun scrollTo(line: Double) {
    top = line
    clamp()
  }

  /** Back to the live tail (after input: show the cursor). */
  fun toBottom() {
    follow = true
    top = liveTop.toDouble()
  }

  /** Keep [top] in range; at or below the live tail the view follows. */
  private fun clamp() {
    if (top > maxTop) top = maxTop.toDouble()
    if (top < minTop) top = minTop.toDouble()
    follow = top >= liveTop
  }
}
