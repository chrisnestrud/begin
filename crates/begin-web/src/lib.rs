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

// ===== multiplayer hosting from the browser =====
//
// The same begin::host::HostCore the native console host runs. The JS
// side owns the lobbylink room and maps player ids to seats 1..N; seat 0
// renders locally. Output JSON:
//   [{"seat":k,"t":"frame","data":"...","flash":false},
//    {"seat":k,"t":"info","text":"..."}, {"seat":k,"t":"over","text":"..."},
//    {"t":"hold","ms":140}, {"t":"ended"}]

use begin::host::{HostCore, HostOut, Msg};
use begin::session::{Setup, SetupResult};
use begin_core::scenario::Scenario;

static SETUP: Mutex<Option<Setup>> = Mutex::new(None);
static SCENARIO: Mutex<Option<(Scenario, String)>> = Mutex::new(None);
static HOST: Mutex<Option<HostCore>> = Mutex::new(None);
static DATA: Mutex<Option<GameData>> = Mutex::new(None);

fn data_handle() -> GameData {
    let mut d = DATA.lock().unwrap();
    if d.is_none() {
        *d = Some(GameData::load());
    }
    d.clone().unwrap()
}

fn encode_host(outs: &[HostOut]) -> Vec<u8> {
    let items: Vec<serde_json::Value> = outs
        .iter()
        .map(|o| match o {
            HostOut::Seat { seat, msg } => match msg {
                Msg::Frame { data, flash } => {
                    serde_json::json!({"seat": seat, "t": "frame", "data": data, "flash": flash})
                }
                Msg::Info(t) => serde_json::json!({"seat": seat, "t": "info", "text": t}),
                Msg::Over(t) => serde_json::json!({"seat": seat, "t": "over", "text": t}),
            },
            HostOut::Hold(ms) => serde_json::json!({"t": "hold", "ms": ms}),
            HostOut::Ended => serde_json::json!({"t": "ended"}),
        })
        .collect();
    serde_json::to_vec(&items).unwrap_or_default()
}

/// Start the fleet-setup dialogue (browser host's pregame). Stages the
/// banner and first prompt as Session-style output.
#[no_mangle]
pub extern "C" fn setup_new(seed_lo: u32, seed_hi: u32) {
    let seed = ((seed_hi as u64) << 32) | seed_lo as u64;
    let _ = data_handle();
    let (setup, outs) = Setup::new(seed, 0.0, None);
    let mut all = banner();
    all.extend(outs);
    *SETUP.lock().unwrap() = Some(setup);
    *SCENARIO.lock().unwrap() = None;
    *RESULT.lock().unwrap() = encode(&all);
}

/// Feed one setup line. When the fleet is configured, the staged output
/// ends with {"k":"go"} (scenario stashed for host_start) or {"k":"quit"}.
///
/// # Safety
/// `ptr` must point to `len` valid bytes.
#[no_mangle]
pub unsafe extern "C" fn setup_feed(ptr: *const u8, len: usize) {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let line = String::from_utf8_lossy(bytes);
    let data = data_handle();
    let mut done = None;
    let outs = match SETUP.lock().unwrap().as_mut() {
        Some(s) => {
            let (outs, result) = s.feed(&data, &line);
            done = result;
            outs
        }
        None => Vec::new(),
    };
    let mut v: Vec<serde_json::Value> =
        serde_json::from_slice(&encode(&outs)).unwrap_or_default();
    match done {
        Some(SetupResult::Go(sc, name)) => {
            *SCENARIO.lock().unwrap() = Some((sc, name));
            v.push(serde_json::json!({"k": "go"}));
        }
        Some(SetupResult::Quit) => v.push(serde_json::json!({"k": "quit"})),
        None => {}
    }
    *RESULT.lock().unwrap() = serde_json::to_vec(&v).unwrap_or_default();
}

/// Create the HostCore once the room is full. `quick` uses the classic
/// duel; otherwise the scenario stashed by setup_feed. `names` is a JSON
/// array of remote seat names in seat order (1..N). Stages host output.
///
/// # Safety
/// `names_ptr` must point to `names_len` valid bytes.
#[no_mangle]
pub unsafe extern "C" fn host_start(
    seed_lo: u32,
    seed_hi: u32,
    quick: u32,
    begin2: u32,
    versus: u32,
    names_ptr: *const u8,
    names_len: usize,
) -> u32 {
    let seed = ((seed_hi as u64) << 32) | seed_lo as u64;
    let names: Vec<String> = {
        let bytes = unsafe { std::slice::from_raw_parts(names_ptr, names_len) };
        serde_json::from_slice(bytes).unwrap_or_default()
    };
    let (scenario, host_name) = if quick != 0 {
        let mut sc = Scenario::duel();
        sc.seed = seed;
        (sc, "Admiral".to_string())
    } else {
        match SCENARIO.lock().unwrap().take() {
            Some(x) => x,
            None => return 1, // setup not completed
        }
    };
    let tuning = if begin2 != 0 { Tuning::begin2() } else { Tuning::default() };
    match HostCore::new(data_handle(), tuning, seed, &scenario, versus != 0, host_name, names) {
        Ok((core, outs)) => {
            *RESULT.lock().unwrap() = encode_host(&outs);
            *HOST.lock().unwrap() = Some(core);
            0
        }
        Err(e) => {
            *RESULT.lock().unwrap() =
                serde_json::to_vec(&serde_json::json!([{ "t": "error", "text": e }]))
                    .unwrap_or_default();
            1
        }
    }
}

/// A line from a seat (0 = the hosting player's own terminal).
///
/// # Safety
/// `ptr` must point to `len` valid bytes.
#[no_mangle]
pub unsafe extern "C" fn host_line(seat: u32, ptr: *const u8, len: usize) {
    let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
    let line = String::from_utf8_lossy(bytes);
    let outs = match HOST.lock().unwrap().as_mut() {
        Some(h) => h.line(seat as usize, &line),
        None => Vec::new(),
    };
    *RESULT.lock().unwrap() = encode_host(&outs);
}

#[no_mangle]
pub extern "C" fn host_disconnect(seat: u32) {
    let outs = match HOST.lock().unwrap().as_mut() {
        Some(h) => h.disconnect(seat as usize),
        None => Vec::new(),
    };
    *RESULT.lock().unwrap() = encode_host(&outs);
}

#[no_mangle]
pub extern "C" fn host_reconnect(seat: u32) {
    let outs = match HOST.lock().unwrap().as_mut() {
        Some(h) => h.reconnect(seat as usize),
        None => Vec::new(),
    };
    *RESULT.lock().unwrap() = encode_host(&outs);
}

#[no_mangle]
pub extern "C" fn host_finished() -> u32 {
    HOST.lock().unwrap().as_ref().map(|h| h.finished() as u32).unwrap_or(1)
}
