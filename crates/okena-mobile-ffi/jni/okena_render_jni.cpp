// JNI shim over okena_render.h for the Android native terminal view.
//
// Compiled into libokena-mobile-ffi.so (the same .so that statically links
// libokena_mobile_ffi.a and hosts the uniffi/JSI module), so these calls reach
// the same Rust ConnectionManager as JS. The source list and include path are
// added by mobile/rn/scripts/fix-android-bindings.cjs after `ubrn build
// android`; never compile this file (or link the Rust .a) into another .so.
//
// Kotlin side (class name is part of the JNI symbol names below):
//
//   package com.okena.terminalview
//   object RustBridge {
//       init { System.loadLibrary("okena-mobile-ffi") }
//       @JvmStatic external fun abiVersion(): Int
//       @JvmStatic external fun terminalCount(connId: String): Int
//       @JvmStatic external fun debugConnections(): String
//
//       // View handle (a jlong owning a C++ Bridge around OkenaRvHandle).
//       @JvmStatic external fun open(connId: String, terminalId: String): Long   // 0 on bad ids
//       @JvmStatic external fun close(handle: Long)
//       @JvmStatic external fun generation(handle: Long): Long
//       // Direct, read-only-by-contract ByteBuffer over the whole frame blob
//       // (see okena_render.h "Frame layout"); set .order(LITTLE_ENDIAN).
//       // Valid until the next snapshot/close on the same handle: the same
//       // ByteBuffer object is reused while the Rust buffer does not move.
//       @JvmStatic external fun snapshot(handle: Long, topLine: Long, rows: Int): java.nio.ByteBuffer?
//       @JvmStatic external fun selectionStart(handle: Long, line: Long, col: Int, side: Int, kind: Int)
//       @JvmStatic external fun selectionUpdate(handle: Long, line: Long, col: Int, side: Int)
//       @JvmStatic external fun selectWord(handle: Long, line: Long, col: Int)
//       @JvmStatic external fun selectionStartWord(handle: Long, line: Long, col: Int): Boolean
//       @JvmStatic external fun setOutputFrozen(handle: Long, frozen: Boolean)
//       @JvmStatic external fun outputFrozen(handle: Long): Boolean
//       // Text of lines start..=end; the anchor line's newline count goes into anchorOut[0].
//       @JvmStatic external fun textRange(handle: Long, start: Long, end: Long, anchor: Long, anchorOut: IntArray): String?
//       @JvmStatic external fun selectionText(handle: Long): String?
//       @JvmStatic external fun selectionClear(handle: Long)
//       @JvmStatic external fun wheel(handle: Long, lines: Int, col: Int, row: Int): Boolean
//       @JvmStatic external fun resizeLocal(handle: Long, cols: Int, rows: Int)
//       @JvmStatic external fun resizeTerminal(handle: Long, cols: Int, rows: Int)
//   }
//
// R8: keep `com.okena.terminalview.RustBridge` and its native methods
// (`-keepclasseswithmembernames class * { native <methods>; }` is in the
// default Android rules; add an explicit -keep if the class gets renamed).
//
// `@JvmStatic` on an `object` member makes the JNI function receive a jclass.
// The library is normally already loaded by the OkenaMobileFfi module;
// loadLibrary is idempotent within the app class loader.

#include <jni.h>

#include <string>
#include <vector>

#include "okena_render.h"

namespace {

// Java strings are UTF-16. NewStringUTF expects *modified* UTF-8 and rejects
// 4-byte sequences (emoji), so decode real UTF-8 ourselves.
jstring utf8_to_jstring(JNIEnv *env, const char *data, size_t len) {
  std::u16string out;
  out.reserve(len);
  size_t i = 0;
  while (i < len) {
    auto b0 = static_cast<unsigned char>(data[i]);
    uint32_t cp = 0xFFFD;
    size_t n = 1;
    if (b0 < 0x80) {
      cp = b0;
    } else if ((b0 >> 5) == 0x6 && i + 1 < len) {
      cp = ((b0 & 0x1F) << 6) | (static_cast<unsigned char>(data[i + 1]) & 0x3F);
      n = 2;
    } else if ((b0 >> 4) == 0xE && i + 2 < len) {
      cp = ((b0 & 0x0F) << 12) |
           ((static_cast<unsigned char>(data[i + 1]) & 0x3F) << 6) |
           (static_cast<unsigned char>(data[i + 2]) & 0x3F);
      n = 3;
    } else if ((b0 >> 3) == 0x1E && i + 3 < len) {
      cp = ((b0 & 0x07) << 18) |
           ((static_cast<unsigned char>(data[i + 1]) & 0x3F) << 12) |
           ((static_cast<unsigned char>(data[i + 2]) & 0x3F) << 6) |
           (static_cast<unsigned char>(data[i + 3]) & 0x3F);
      n = 4;
    }
    i += n;
    if (cp >= 0x10000) {
      cp -= 0x10000;
      out.push_back(static_cast<char16_t>(0xD800 + (cp >> 10)));
      out.push_back(static_cast<char16_t>(0xDC00 + (cp & 0x3FF)));
    } else {
      out.push_back(static_cast<char16_t>(cp));
    }
  }
  return env->NewString(reinterpret_cast<const jchar *>(out.data()),
                        static_cast<jsize>(out.size()));
}

// Borrowed UTF-8 view of a Java string (ids are ASCII, so modified UTF-8 is
// fine for input).
class JStringUtf {
 public:
  JStringUtf(JNIEnv *env, jstring s) : env_(env), s_(s) {
    chars_ = s ? env->GetStringUTFChars(s, nullptr) : nullptr;
  }
  ~JStringUtf() {
    if (chars_) env_->ReleaseStringUTFChars(s_, chars_);
  }
  JStringUtf(const JStringUtf &) = delete;
  JStringUtf &operator=(const JStringUtf &) = delete;
  const char *c_str() const { return chars_; }

 private:
  JNIEnv *env_;
  jstring s_;
  const char *chars_;
};

// Owns the Rust handle plus the cached direct ByteBuffer over its frame.
struct Bridge {
  OkenaRvHandle *handle;
  jobject buffer;     // global ref or null
  const void *ptr;    // base address the buffer wraps
  jlong capacity;     // its capacity in bytes
};

Bridge *bridge(jlong h) { return reinterpret_cast<Bridge *>(static_cast<intptr_t>(h)); }

OkenaRvHandle *rv(jlong h) {
  Bridge *b = bridge(h);
  return b ? b->handle : nullptr;
}

uint16_t u16(jint v) {
  return static_cast<uint16_t>(v < 0 ? 0 : (v > 0xFFFF ? 0xFFFF : v));
}

}  // namespace

#define RB(name) Java_com_okena_terminalview_RustBridge_##name

extern "C" {

JNIEXPORT jint JNICALL RB(abiVersion)(JNIEnv *, jclass) {
  return static_cast<jint>(okena_rv_abi_version());
}

JNIEXPORT jint JNICALL RB(terminalCount)(JNIEnv *env, jclass, jstring conn_id) {
  JStringUtf id(env, conn_id);
  return okena_rv_terminal_count(id.c_str());
}

JNIEXPORT jstring JNICALL RB(debugConnections)(JNIEnv *env, jclass) {
  std::vector<char> buf(1024);
  size_t len = okena_rv_debug_connections(buf.data(), buf.size());
  if (len >= buf.size()) {
    buf.resize(len + 1);
    len = okena_rv_debug_connections(buf.data(), buf.size());
  }
  return utf8_to_jstring(env, buf.data(), len < buf.size() ? len : buf.size() - 1);
}

JNIEXPORT jlong JNICALL RB(open)(JNIEnv *env, jclass, jstring conn_id, jstring terminal_id) {
  JStringUtf conn(env, conn_id);
  JStringUtf term(env, terminal_id);
  OkenaRvHandle *h = okena_rv_open(conn.c_str(), term.c_str());
  if (!h) return 0;
  auto *b = new Bridge{h, nullptr, nullptr, 0};
  return static_cast<jlong>(reinterpret_cast<intptr_t>(b));
}

JNIEXPORT void JNICALL RB(close)(JNIEnv *env, jclass, jlong h) {
  Bridge *b = bridge(h);
  if (!b) return;
  if (b->buffer) env->DeleteGlobalRef(b->buffer);
  okena_rv_close(b->handle);
  delete b;
}

JNIEXPORT jlong JNICALL RB(generation)(JNIEnv *, jclass, jlong h) {
  return static_cast<jlong>(okena_rv_generation(rv(h)));
}

JNIEXPORT jobject JNICALL RB(snapshot)(JNIEnv *env, jclass, jlong h, jlong top_line, jint rows) {
  Bridge *b = bridge(h);
  if (!b) return nullptr;
  const OkenaRvFrame *f = okena_rv_snapshot(b->handle, static_cast<int64_t>(top_line), u16(rows));
  if (!f) return nullptr;
  jlong size = static_cast<jlong>(f->total_size);
  if (!b->buffer || b->ptr != f || b->capacity < size) {
    if (b->buffer) env->DeleteGlobalRef(b->buffer);
    jobject local = env->NewDirectByteBuffer(const_cast<OkenaRvFrame *>(f), size);
    b->buffer = local ? env->NewGlobalRef(local) : nullptr;
    if (local) env->DeleteLocalRef(local);
    b->ptr = f;
    b->capacity = size;
  }
  return b->buffer ? env->NewLocalRef(b->buffer) : nullptr;
}

JNIEXPORT void JNICALL RB(selectionStart)(JNIEnv *, jclass, jlong h, jlong line, jint col,
                                          jint side, jint kind) {
  okena_rv_selection_start(rv(h), static_cast<int64_t>(line), u16(col),
                           static_cast<uint8_t>(side), static_cast<uint8_t>(kind));
}

JNIEXPORT void JNICALL RB(selectionUpdate)(JNIEnv *, jclass, jlong h, jlong line, jint col,
                                           jint side) {
  okena_rv_selection_update(rv(h), static_cast<int64_t>(line), u16(col),
                            static_cast<uint8_t>(side));
}

JNIEXPORT void JNICALL RB(selectWord)(JNIEnv *, jclass, jlong h, jlong line, jint col) {
  okena_rv_select_word(rv(h), static_cast<int64_t>(line), u16(col));
}

JNIEXPORT jboolean JNICALL RB(selectionStartWord)(JNIEnv *, jclass, jlong h, jlong line,
                                                  jint col) {
  return okena_rv_selection_start_word(rv(h), static_cast<int64_t>(line), u16(col)) != 0
             ? JNI_TRUE
             : JNI_FALSE;
}

JNIEXPORT void JNICALL RB(setOutputFrozen)(JNIEnv *, jclass, jlong h, jboolean frozen) {
  okena_rv_set_output_frozen(rv(h), frozen ? 1 : 0);
}

JNIEXPORT jboolean JNICALL RB(outputFrozen)(JNIEnv *, jclass, jlong h) {
  return okena_rv_output_frozen(rv(h)) != 0 ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT jstring JNICALL RB(selectionText)(JNIEnv *env, jclass, jlong h) {
  size_t len = 0;
  const char *text = okena_rv_selection_text(rv(h), &len);
  return text ? utf8_to_jstring(env, text, len) : nullptr;
}

JNIEXPORT void JNICALL RB(selectionClear)(JNIEnv *, jclass, jlong h) {
  okena_rv_selection_clear(rv(h));
}

JNIEXPORT jstring JNICALL RB(textRange)(JNIEnv *env, jclass, jlong h, jlong start, jlong end,
                                       jlong anchor, jintArray anchor_out) {
  size_t len = 0;
  uint32_t anchor_newlines = 0;
  const char *text = okena_rv_text_range(rv(h), static_cast<int64_t>(start),
                                         static_cast<int64_t>(end), static_cast<int64_t>(anchor),
                                         &len, &anchor_newlines);
  if (!text) return nullptr;
  if (anchor_out && env->GetArrayLength(anchor_out) > 0) {
    jint value = static_cast<jint>(anchor_newlines);
    env->SetIntArrayRegion(anchor_out, 0, 1, &value);
  }
  return utf8_to_jstring(env, text, len);
}

JNIEXPORT jboolean JNICALL RB(wheel)(JNIEnv *, jclass, jlong h, jint lines, jint col, jint row) {
  return okena_rv_wheel(rv(h), lines, u16(col), u16(row)) != 0 ? JNI_TRUE : JNI_FALSE;
}

JNIEXPORT void JNICALL RB(resizeLocal)(JNIEnv *, jclass, jlong h, jint cols, jint rows) {
  okena_rv_resize_local(rv(h), u16(cols), u16(rows));
}

JNIEXPORT void JNICALL RB(resizeTerminal)(JNIEnv *, jclass, jlong h, jint cols, jint rows) {
  okena_rv_resize_terminal(rv(h), u16(cols), u16(rows));
}

}  // extern "C"
