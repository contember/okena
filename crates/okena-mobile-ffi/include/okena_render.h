/*
 * okena_render.h — C ABI between the Rust core (crates/okena-mobile-ffi,
 * src/render.rs) and the native terminal views (Swift on iOS, C++/JNI and
 * Kotlin on Android).
 *
 * This header is the single contract. Every function is implemented in
 * render.rs with `#[unsafe(no_mangle)] extern "C"`; keep both in sync and bump
 * OKENA_RV_ABI_VERSION on any incompatible change.
 *
 * ── Linking (one Rust copy) ───────────────────────────────────────────────
 * The functions live in the same Rust library as the uniffi/JSI module, so
 * they see the same ConnectionManager singleton and the connections that JS
 * opened. Never link a second copy of the Rust library:
 *   - iOS: the static library in OkenaMobileFfiFramework.xcframework is
 *     linked once into the app by the OkenaMobileFfi pod. A Swift pod only
 *     adds this directory to SWIFT_INCLUDE_PATHS and does `import OkenaRender`
 *     (module.modulemap next to this file). Symbols resolve at app link time.
 *   - Android: libokena_mobile_ffi.a is linked into libokena-mobile-ffi.so.
 *     The JNI shim crates/okena-mobile-ffi/jni/okena_render_jni.cpp is
 *     compiled into that same .so (added by
 *     mobile/rn/scripts/fix-android-bindings.cjs after `ubrn build android`).
 *     Kotlin talks to it through the class
 *         com.okena.terminalview.RustBridge
 *     whose `external` methods are listed in okena_render_jni.cpp.
 *
 * ── Threading ─────────────────────────────────────────────────────────────
 * The free functions are thread-safe. A handle (from a later ABI) belongs to
 * one thread at a time.
 *
 * ── Strings ───────────────────────────────────────────────────────────────
 * Input strings are NUL-terminated UTF-8, borrowed for the duration of the
 * call. No function keeps a pointer to them.
 */
#ifndef OKENA_RENDER_H
#define OKENA_RENDER_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OKENA_RV_ABI_VERSION 1u

/* ABI version of the linked library. Compare with OKENA_RV_ABI_VERSION. */
uint32_t okena_rv_abi_version(void);

/*
 * Number of terminals the connection `conn_id` currently mirrors, or -1 when
 * the connection does not exist (or init_app has not run yet).
 */
int32_t okena_rv_terminal_count(const char *conn_id);

/*
 * Diagnostic listing of the live connections, one line each:
 *   "<conn_id>\t<terminal_count>\t<term_id>,<term_id>,...\n"
 * Writes at most cap-1 bytes plus a NUL into buf (buf may be NULL when cap is
 * 0) and returns the full length without the NUL. If the return value is >=
 * cap, the text was truncated; call again with a larger buffer.
 */
size_t okena_rv_debug_connections(char *buf, size_t cap);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* OKENA_RENDER_H */
