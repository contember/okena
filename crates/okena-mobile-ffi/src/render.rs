//! Hand-written C ABI (`okena_rv_*`) for the native terminal views.
//!
//! The Swift (iOS) and Kotlin/JNI (Android) terminal views call these
//! functions directly instead of going through the uniffi/JSI module. They run
//! in the same Rust copy as the JSI module (one static library linked once into
//! the app binary on iOS, into `libokena-mobile-ffi.so` on Android), so they
//! see the same `ConnectionManager` singleton and the connections JS opened.
//!
//! The contract is `crates/okena-mobile-ffi/include/okena_render.h`. Keep the
//! two in sync: every `#[unsafe(no_mangle)]` function here has a prototype there.
//!
//! No function here may unwind into the caller. Every entry point goes through
//! [`guard`], which turns a panic into the documented fallback value.

use std::ffi::{CStr, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::client::manager::ConnectionManager;

/// Bumped on every incompatible change to `okena_render.h`.
pub const ABI_VERSION: u32 = 1;

/// Run `f`, returning `fallback` if it panics.
fn guard<R>(fallback: R, f: impl FnOnce() -> R) -> R {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or(fallback)
}

/// Borrow a NUL-terminated UTF-8 string from the caller.
///
/// # Safety
/// `ptr` must be null or point to a NUL-terminated string that outlives `'a`.
unsafe fn borrow_str<'a>(ptr: *const c_char) -> Option<&'a str> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated per the caller contract.
    unsafe { CStr::from_ptr(ptr) }.to_str().ok()
}

/// Copy `text` into a caller buffer of `cap` bytes, NUL-terminated and
/// truncated if needed. Returns `text.len()`, so a caller can retry with a
/// larger buffer.
///
/// # Safety
/// `buf` must be null or valid for `cap` bytes of writes.
unsafe fn write_c_buf(text: &str, buf: *mut c_char, cap: usize) -> usize {
    if !buf.is_null() && cap > 0 {
        let n = text.len().min(cap - 1);
        // SAFETY: `buf` is valid for `cap` bytes and `n + 1 <= cap`.
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), buf.cast::<u8>(), n);
            *buf.add(n) = 0;
        }
    }
    text.len()
}

/// ABI version of this library; native code compares it with
/// `OKENA_RV_ABI_VERSION` from the header it was compiled against.
#[unsafe(no_mangle)]
pub extern "C" fn okena_rv_abi_version() -> u32 {
    ABI_VERSION
}

/// Number of terminals the connection `conn_id` mirrors, or -1 if the
/// connection does not exist (or `init_app` has not run yet).
///
/// # Safety
/// `conn_id` must be null or a NUL-terminated string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_terminal_count(conn_id: *const c_char) -> i32 {
    guard(-1, || {
        // SAFETY: forwarded caller contract.
        let Some(conn_id) = (unsafe { borrow_str(conn_id) }) else {
            return -1;
        };
        ConnectionManager::try_get()
            .and_then(|mgr| mgr.terminal_ids(conn_id))
            .map_or(-1, |ids| i32::try_from(ids.len()).unwrap_or(i32::MAX))
    })
}

/// Diagnostic listing of the live connections, one line each:
/// `<conn_id>\t<terminal_count>\t<term_id>,<term_id>,...\n`.
///
/// Writes at most `cap - 1` bytes plus a NUL into `buf` and returns the full
/// length (without NUL); call again with a larger buffer if it was truncated.
///
/// # Safety
/// `buf` must be null or valid for `cap` bytes of writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn okena_rv_debug_connections(buf: *mut c_char, cap: usize) -> usize {
    guard(0, || {
        let text = connections_listing();
        // SAFETY: forwarded caller contract.
        unsafe { write_c_buf(&text, buf, cap) }
    })
}

fn connections_listing() -> String {
    let Some(mgr) = ConnectionManager::try_get() else {
        return String::new();
    };
    let mut ids = mgr.connection_ids();
    ids.sort();
    let mut out = String::new();
    for conn_id in ids {
        let terms = mgr.terminal_ids(&conn_id).unwrap_or_default();
        out.push_str(&format!(
            "{conn_id}\t{}\t{}\n",
            terms.len(),
            terms.join(",")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_c_buf_truncates_and_terminates() {
        let mut buf = [0x7f_i8; 4];
        let n = unsafe { write_c_buf("hello", buf.as_mut_ptr(), buf.len()) };
        assert_eq!(n, 5);
        assert_eq!(unsafe { CStr::from_ptr(buf.as_ptr()) }.to_str(), Ok("hel"));
    }

    #[test]
    fn unknown_connection_counts_minus_one() {
        ConnectionManager::init();
        let id = c"no-such-connection";
        assert_eq!(unsafe { okena_rv_terminal_count(id.as_ptr()) }, -1);
        assert_eq!(unsafe { okena_rv_terminal_count(std::ptr::null()) }, -1);
    }
}
