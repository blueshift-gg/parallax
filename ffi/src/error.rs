//! Status codes and the thread-local last-error string.
//!
//! Every extern function returns an `i32` status: [`PARALLAX_OK`] (0) on
//! success, a negative code on failure. On failure the function also stores a
//! human-readable message retrievable with `parallax_last_error`; the pointer
//! is valid until the next FFI call on the same thread.
//!
//! # The last-error channel is per-thread
//!
//! The last-error string lives in a `thread_local!`, so `parallax_last_error`
//! reports only the error from the most recent FFI call *on the calling
//! thread*. A C caller driving several worlds from several threads must read
//! `parallax_last_error` on the same thread that made the failing call, before
//! that thread makes another FFI call — an error raised on one thread is
//! invisible to every other. Within one thread this is exactly the
//! documented "valid until the next FFI call" contract; across threads the
//! channels are independent. (The harness itself pins each world to one
//! thread; see [`crate::ffi::parallax_new`].)

use std::cell::RefCell;
use std::ffi::CString;
use std::os::raw::c_char;

/// The call succeeded.
pub const PARALLAX_OK: i32 = 0;
/// A required pointer argument was null.
pub const PARALLAX_ERR_NULL_POINTER: i32 = -1;
/// A wire buffer could not be decoded (malformed or truncated).
pub const PARALLAX_ERR_INVALID_WIRE: i32 = -2;
/// A program artifact could not be loaded as a valid SBF ELF.
pub const PARALLAX_ERR_PROGRAM_LOAD: i32 = -3;
/// Transaction execution set-up failed (before the runtime ran).
pub const PARALLAX_ERR_EXECUTION: i32 = -4;
/// A dump or dump-file operation failed: an RPC response could not be
/// applied, the `.parallax/` store could not be read or written, or a dump
/// file was missing or malformed.
pub const PARALLAX_ERR_DUMP: i32 = -6;
/// A world handle was used from a thread other than the one that created it.
/// Each world is owned by its creating thread; see [`crate::ffi::parallax_new`].
pub const PARALLAX_ERR_WRONG_THREAD: i32 = -5;
/// A Rust panic was caught at the boundary.
pub const PARALLAX_ERR_INTERNAL: i32 = -99;

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

/// Store `msg` as the current thread's last error.
pub fn set_last_error(msg: impl Into<String>) {
    // An interior NUL must not drop the message (the contract promises one for
    // every failure); escape it instead.
    let msg = msg.into().replace('\0', "\\0");
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = CString::new(msg).ok();
    });
}

/// Clear the current thread's last error at the start of a call.
pub fn clear_last_error() {
    LAST_ERROR.with(|cell| {
        *cell.borrow_mut() = None;
    });
}

/// A borrowed pointer to the current thread's last-error C string, or null.
pub fn last_error_ptr() -> *const c_char {
    LAST_ERROR.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|s| s.as_ptr())
            .unwrap_or(std::ptr::null())
    })
}
