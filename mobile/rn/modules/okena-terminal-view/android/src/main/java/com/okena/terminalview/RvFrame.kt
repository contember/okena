package com.okena.terminalview

import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * Reader over one frame blob from [RustBridge.snapshot]. Field offsets mirror `OkenaRvFrame`,
 * `OkenaRvRow` and `OkenaRvCell` in `okena_render.h` (the contract); keep them in sync.
 *
 * [load] copies the header into plain fields, so overlays and the scroll model can use them
 * after the buffer is gone. Row and cell accessors read the buffer in place and are valid only
 * until the next snapshot on the same handle.
 */
internal class RvFrame {
  var buf: ByteBuffer? = null
    private set

  var status = STATUS_MISSING
  var generation = 0L
  var topLine = 0L
  var firstLine = 0L
  var screenTopLine = 0L
  var cursorLine = 0L
  var selStartLine = 0L
  var selEndLine = 0L
  var historySize = 0
  var mode = 0
  var reflowEpoch = 0
  var defaultFg = 0
  var defaultBg = 0
  var cols = 0
  var rows = 0
  var screenLines = 0
  var cursorCol = 0
  var selStartCol = 0
  var selEndCol = 0
  var cursorShape = 0
  var cursorVisible = false
  var selKind = SEL_NONE
  private var rowsOffset = 0
  private var cellsOffset = 0
  private var extrasOffset = 0
  private var extrasCount = 0
  private var textOffset = 0

  /** Last line of the live screen (absolute). */
  val lastLine: Long get() = screenTopLine + screenLines - 1

  fun load(buffer: ByteBuffer?): Boolean {
    if (buffer == null || buffer.capacity() < HEADER_SIZE) {
      buf = null
      status = STATUS_MISSING
      rows = 0
      cols = 0
      return false
    }
    val b = buffer.order(ByteOrder.LITTLE_ENDIAN)
    buf = b
    status = b.getInt(12)
    generation = b.getLong(16)
    topLine = b.getLong(24)
    firstLine = b.getLong(32)
    screenTopLine = b.getLong(40)
    cursorLine = b.getLong(48)
    selStartLine = b.getLong(56)
    selEndLine = b.getLong(64)
    historySize = b.getInt(72)
    mode = b.getInt(76)
    reflowEpoch = b.getInt(80)
    defaultFg = b.getInt(84)
    defaultBg = b.getInt(88)
    cols = u16(b, 92)
    rows = u16(b, 94)
    screenLines = u16(b, 96)
    cursorCol = u16(b, 98)
    selStartCol = u16(b, 100)
    selEndCol = u16(b, 102)
    cursorShape = b.get(104).toInt() and 0xff
    cursorVisible = b.get(105).toInt() != 0
    selKind = b.get(106).toInt() and 0xff
    rowsOffset = b.getInt(108)
    cellsOffset = b.getInt(112)
    extrasOffset = b.getInt(116)
    extrasCount = b.getInt(120)
    textOffset = b.getInt(124)
    if (status != STATUS_OK) {
      rows = 0
      cols = 0
    }
    return true
  }

  // ── Rows ────────────────────────────────────────────────────────────────

  fun rowLine(r: Int): Long = buf!!.getLong(rowsOffset + r * ROW_SIZE)

  fun rowHash(r: Int): Long = buf!!.getLong(rowsOffset + r * ROW_SIZE + 8)

  fun rowFlags(r: Int): Int = buf!!.getInt(rowsOffset + r * ROW_SIZE + 16)

  // ── Cells ───────────────────────────────────────────────────────────────

  /** Byte offset of cell (r, c); pass it to the cell accessors below. */
  fun cellAt(r: Int, c: Int): Int = cellsOffset + (r * cols + c) * CELL_SIZE

  fun codepoint(cell: Int): Int = buf!!.getInt(cell)

  fun fg(cell: Int): Int = buf!!.getInt(cell + 4)

  fun bg(cell: Int): Int = buf!!.getInt(cell + 8)

  fun flags(cell: Int): Int = buf!!.getShort(cell + 12).toInt() and 0xffff

  fun extra(cell: Int): Int = buf!!.getShort(cell + 14).toInt() and 0xffff

  /** The full grapheme of a cell with `extra != 0`, or null for a bad index. */
  fun extraText(extra: Int): String? {
    if (extra == 0 || extra > extrasCount) return null
    val b = buf!!
    val e = extrasOffset + (extra - 1) * EXTRA_SIZE
    val off = b.getInt(e)
    val len = b.getInt(e + 4)
    if (len <= 0) return null
    val bytes = ByteArray(len)
    val dup = b.duplicate()
    dup.position(textOffset + off)
    dup.get(bytes, 0, len)
    return String(bytes, Charsets.UTF_8)
  }

  companion object {
    const val HEADER_SIZE = 136
    const val ROW_SIZE = 24
    const val CELL_SIZE = 16
    const val EXTRA_SIZE = 8

    const val STATUS_OK = 0
    const val STATUS_MISSING = 1

    const val CELL_BOLD = 1 shl 0
    const val CELL_ITALIC = 1 shl 1
    const val CELL_UNDERLINE = 1 shl 2
    const val CELL_STRIKE = 1 shl 3
    const val CELL_INVERSE = 1 shl 4
    const val CELL_DIM = 1 shl 5
    const val CELL_WIDE = 1 shl 6
    const val CELL_WIDE_SPACER = 1 shl 7
    const val CELL_LEADING_WIDE_SPACER = 1 shl 8
    const val CELL_HIDDEN = 1 shl 9
    const val CELL_DOUBLE_UNDERLINE = 1 shl 10
    const val CELL_UNDERCURL = 1 shl 11
    const val CELL_DOTTED_UNDERLINE = 1 shl 12
    const val CELL_DASHED_UNDERLINE = 1 shl 13

    const val ROW_VOID = 1 shl 0
    const val ROW_WRAPPED = 1 shl 1

    const val MODE_MOUSE = 1 shl 0
    const val MODE_ALT_SCREEN = 1 shl 1
    const val MODE_ALTERNATE_SCROLL = 1 shl 2

    const val CURSOR_BLOCK = 0
    const val CURSOR_UNDERLINE = 1
    const val CURSOR_BEAM = 2
    const val CURSOR_HOLLOW_BLOCK = 3

    const val SEL_NONE = 0
    const val SEL_STREAM = 1
    const val SEL_BLOCK = 2

    const val SELECT_SIMPLE = 0
    const val SELECT_WORD = 1

    const val SIDE_LEFT = 0
    const val SIDE_RIGHT = 1

    private fun u16(b: ByteBuffer, off: Int): Int = b.getShort(off).toInt() and 0xffff
  }
}
