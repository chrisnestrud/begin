//! BEGIN in the browser: a thin C-ABI wasm wrapper around
//! [`begin::session::Session`] — the exact state machine the native console
//! runs, so browser and console play identically.
//!
//! Protocol (see `web/begin.js` for the JS side):
//! - `begin_new(seed_lo, seed_hi, quick, begin2, planar, no_flash)` starts a
//!   session and stages its opening output.
//! - `begin_feed(ptr, len)` feeds one UTF-8 input line and stages the output.
//! - Staged output is JSON: `[{"k":"frame","ansi":"...","flash":false},
//!   {"k":"text","t":"..."}, {"k":"prompt","t":"..."}, {"k":"hold","ms":140},
//!   {"k":"done"}]`; read it from `begin_result_ptr()` /
//!   `begin_result_len()` after each call.
//! - `begin_alloc(len)` / `begin_dealloc(ptr, len)` manage the input buffer.

use begin::session::{banner, Out, Session, SessionOpts};
use begin_core::{GameData, Tuning};
use std::sync::Mutex;

static SESSION: Mutex<Option<Session>> = Mutex::new(None);
static RESULT: Mutex<Vec<u8>> = Mutex::new(Vec::new());

fn encode(outs: &[Out]) -> Vec<u8> {
    let items: Vec<serde_json::Value> = outs
        .iter()
        .map(|o| match o {
            Out::Frame { ansi, flash } => {
                serde_json::json!({"k": "frame", "ansi": ansi, "flash": flash})
            }
            Out::Text(t) => serde_json::json!({"k": "text", "t": t}),
            Out::Prompt(p) => serde_json::json!({"k": "prompt", "t": p}),
            Out::Hold(ms) => serde_json::json!({"k": "hold", "ms": ms}),
            Out::Done => serde_json::json!({"k": "done"}),
        })
        .collect();
    serde_json::to_vec(&items).unwrap_or_default()
}

/// Start a new session; stages the banner + opening output.
#[no_mangle]
pub extern "C" fn begin_new(
    seed_lo: u32,
    seed_hi: u32,
    quick: u32,
    begin2: u32,
    planar: u32,
    no_flash: u32,
) {
    let seed = ((seed_hi as u64) << 32) | seed_lo as u64;
    let mut tuning = if begin2 != 0 { Tuning::begin2() } else { Tuning::default() };
    tuning.planar_lock = planar != 0;
    let opts = SessionOpts {
        tuning,
        seed,
        epoch_jd: 0.0,
        spawn_body: None,
        flash: no_flash == 0,
        quick: quick != 0,
    };
    let (sess, outs) = Session::new(GameData::load(), opts);
    let mut all = banner();
    all.extend(outs);
    *SESSION.lock().unwrap() = Some(sess);
    *RESULT.lock().unwrap() = encode(&all);
}

/// Feed one input line (UTF-8 at ptr..ptr+len); stages the output.
///
/// # Safety
/// `ptr` must point to `len` bytes previously obtained from `begin_alloc`
/// (or otherwise valid wasm memory owned by the caller).
#[no_mangle]
pub unsafe extern "C" fn begin_feed(ptr: *const u8, len: usize) {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let line = String::from_utf8_lossy(bytes);
    let outs = match SESSION.lock().unwrap().as_mut() {
        Some(s) => s.feed(&line),
        None => Vec::new(),
    };
    *RESULT.lock().unwrap() = encode(&outs);
}

/// Whether the session has ended (after "done" was emitted).
#[no_mangle]
pub extern "C" fn begin_finished() -> u32 {
    SESSION.lock().unwrap().as_ref().map(|s| s.finished() as u32).unwrap_or(1)
}

#[no_mangle]
pub extern "C" fn begin_result_ptr() -> *const u8 {
    RESULT.lock().unwrap().as_ptr()
}

#[no_mangle]
pub extern "C" fn begin_result_len() -> usize {
    RESULT.lock().unwrap().len()
}

/// Allocate an input buffer the JS side can write into.
#[no_mangle]
pub extern "C" fn begin_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    ptr
}

/// Free a buffer from `begin_alloc`.
///
/// # Safety
/// `ptr`/`len` must match a prior `begin_alloc(len)` exactly.
#[no_mangle]
pub unsafe extern "C" fn begin_dealloc(ptr: *mut u8, len: usize) {
    unsafe {
        drop(Vec::from_raw_parts(ptr, 0, len.max(1)));
    }
}
