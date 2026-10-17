package com.okena.terminalview

/**
 * The view's scroll position in the line-addressing model of `okena_render.h`.
 *
 * [top] is the absolute line at the view's top edge, fractional for sub-line pixel scrolling.
 * The readable range is `[minTop, maxTop]` with `minTop = first_line` and
 * `maxTop = screen_top_line` (the live tail: the screen's top row at the view's top edge), or
 * lower when the grid has more rows than the view.
 *
 *   - At the live tail ([follow]) the view tracks `screen_top_line` as output streams.
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
   * rows the view shows: when the grid is taller than the view (the server kept a larger PTY
   * size), the live tail shows the bottom of the grid, where the cursor usually is.
   */
  fun update(firstLine: Long, screenTopLine: Long, screenLines: Int, viewRows: Int, reflowEpoch: Int): Boolean {
    val tail = screenTopLine + maxOf(0, screenLines - viewRows)
    val before = top
    if (!known) {
      known = true
      follow = true
    } else if (reflowEpoch != epoch && !follow) {
      top = tail - (maxTop - top)
    }
    epoch = reflowEpoch
    minTop = minOf(firstLine, tail)
    maxTop = tail
    if (follow) top = maxTop.toDouble()
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

  fun toBottom() {
    follow = true
    top = maxTop.toDouble()
  }

  private fun clamp() {
    if (top < minTop) top = minTop.toDouble()
    if (top >= maxTop) {
      top = maxTop.toDouble()
      follow = true
    } else {
      follow = false
    }
  }
}
