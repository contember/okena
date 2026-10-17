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
//   }
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

}  // extern "C"
