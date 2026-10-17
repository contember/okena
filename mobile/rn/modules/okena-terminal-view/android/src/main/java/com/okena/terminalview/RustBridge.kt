package com.okena.terminalview

import java.nio.ByteBuffer

/**
 * JNI entry points of the Rust render ABI (`crates/okena-mobile-ffi/include/okena_render.h`).
 *
 * The native side is `crates/okena-mobile-ffi/jni/okena_render_jni.cpp`, compiled into
 * `libokena-mobile-ffi.so`: the same library that hosts the uniffi/JSI module, so these calls
 * reach the same Rust `ConnectionManager` as JS. The class and method names are part of the JNI
 * symbol names; do not rename them. If R8 is ever enabled, keep this class and its natives.
 *
 * A handle (a `Long`) belongs to one thread, the UI thread here. The buffer returned by
 * [snapshot] is valid only until the next [snapshot] or [close] on the same handle.
 */
object RustBridge {
  init {
    System.loadLibrary("okena-mobile-ffi")
  }

  @JvmStatic external fun abiVersion(): Int

  @JvmStatic external fun terminalCount(connId: String): Int

  @JvmStatic external fun debugConnections(): String

  /** 0 on bad ids. Works before the terminal exists (frames then report MISSING). */
  @JvmStatic external fun open(connId: String, terminalId: String): Long

  @JvmStatic external fun close(handle: Long)

  /** 0 while the terminal is missing. */
  @JvmStatic external fun generation(handle: Long): Long

  /** Direct buffer over the Rust-owned frame. Set `.order(LITTLE_ENDIAN)`; never keep it. */
  @JvmStatic external fun snapshot(handle: Long, topLine: Long, rows: Int): ByteBuffer?

  @JvmStatic external fun selectionStart(handle: Long, line: Long, col: Int, side: Int, kind: Int)

  @JvmStatic external fun selectionUpdate(handle: Long, line: Long, col: Int, side: Int)

  @JvmStatic external fun selectWord(handle: Long, line: Long, col: Int)

  @JvmStatic external fun selectionText(handle: Long): String?

  @JvmStatic external fun selectionClear(handle: Long)

  /** True when the app consumed the notches (mouse mode, or alt screen with 1007). */
  @JvmStatic external fun wheel(handle: Long, lines: Int, col: Int, row: Int): Boolean

  @JvmStatic external fun resizeLocal(handle: Long, cols: Int, rows: Int)

  @JvmStatic external fun resizeTerminal(handle: Long, cols: Int, rows: Int)
}
