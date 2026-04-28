//! `@narenana/blackbox-parser` — thin WASM facade over the `blackbox-log`
//! Rust crate. Designed to be dropped into any browser-side project that
//! needs to read Betaflight / iNAV / Cleanflight blackbox binary logs.
//!
//! ## Architecture
//!
//! `parseBlackbox(bytes, mainStride)` returns a `FlightLog` struct that the
//! JS side reads via getters. The bulk numeric data — main / slow / GPS
//! frame values plus their timestamp arrays — comes back as
//! `Float64Array`s, transferred as a single contiguous buffer per stream
//! rather than as nested JS arrays.
//!
//! The earlier prototype used `serde-wasm-bindgen` to serialize a nested
//! `Vec<Vec<f64>>`. That approach allocated one JsValue per number, which
//! hung the worker for minutes on multi-megabyte logs (millions of
//! per-element heap allocations across the WASM/JS boundary). Switching
//! to flat `Vec<f64>` exposed as `Float64Array` cuts that to one memcpy
//! per stream — orders of magnitude faster and matches what the
//! Betaflight reference viewer does in pure JS.
//!
//! ## Frame layout
//!
//! Frame data is row-major. Field `j` of row `i` is `frames[i * cols + j]`,
//! where `cols` matches the corresponding `*FieldNames` array length. The
//! `*Signed` array is a `Uint8Array` of `0`/`1` flags — values from a
//! "signed" column should be interpreted as `i32` rather than `u32` before
//! any unit conversion. (The parser flattens both to `f64` here; the
//! signedness flag is only relevant if a consumer wants to recover the
//! original integer interpretation.)
//!
//! ## Lifetime gymnastics, sidestepped
//!
//! `blackbox-log`'s parser API is `File<'data> -> Headers<'data> ->
//! DataParser<'data, 'headers>`, three lifetimes deep. wasm-bindgen can't
//! carry borrows across the JS boundary, so a streaming wrapper would need
//! `ouroboros` or `yoke` to own a self-referential parser. We bypass all
//! that here by parsing eagerly inside one function call — bytes go in,
//! plain owned `Vec`s come out, no lifetimes leak across the FFI seam.

use blackbox_log::frame::{Frame, FrameDef};
use blackbox_log::prelude::*;
use js_sys::Float64Array;
use wasm_bindgen::prelude::*;

/// JS-facing handle to a parsed blackbox log. The bulk frame data lives in
/// owned Rust `Vec<f64>` buffers; the getter methods produce
/// `Float64Array`s on demand by copying the buffer into JS heap memory.
/// Call `.free()` from JS once the data has been read so we drop the
/// Rust-side allocations.
#[wasm_bindgen]
pub struct FlightLog {
    firmware: String,
    craft_name: String,
    board_info: String,
    has_gps: bool,

    main_field_names: Vec<String>,
    main_field_signed: Vec<u8>,
    main_field_units: Vec<String>,
    main_cols: usize,
    main_times: Vec<f64>,
    main_frames: Vec<f64>, // row-major: frame[i][j] = main_frames[i*main_cols + j]

    slow_field_names: Vec<String>,
    slow_field_signed: Vec<u8>,
    slow_field_units: Vec<String>,
    slow_cols: usize,
    slow_times: Vec<f64>,
    slow_frames: Vec<f64>,

    gps_field_names: Vec<String>,
    gps_field_signed: Vec<u8>,
    gps_field_units: Vec<String>,
    gps_cols: usize,
    gps_times: Vec<f64>,
    gps_frames: Vec<f64>,
}

#[wasm_bindgen]
impl FlightLog {
    #[wasm_bindgen(getter)]
    pub fn firmware(&self) -> String {
        self.firmware.clone()
    }
    #[wasm_bindgen(getter, js_name = "craftName")]
    pub fn craft_name(&self) -> String {
        self.craft_name.clone()
    }
    #[wasm_bindgen(getter, js_name = "boardInfo")]
    pub fn board_info(&self) -> String {
        self.board_info.clone()
    }
    #[wasm_bindgen(getter, js_name = "hasGps")]
    pub fn has_gps(&self) -> bool {
        self.has_gps
    }

    // ── Main frames ──────────────────────────────────────────────────
    #[wasm_bindgen(getter, js_name = "mainFieldNames")]
    pub fn main_field_names(&self) -> Vec<String> {
        self.main_field_names.clone()
    }
    #[wasm_bindgen(getter, js_name = "mainFieldSigned")]
    pub fn main_field_signed(&self) -> Vec<u8> {
        self.main_field_signed.clone()
    }
    #[wasm_bindgen(getter, js_name = "mainFieldUnits")]
    pub fn main_field_units(&self) -> Vec<String> {
        self.main_field_units.clone()
    }
    #[wasm_bindgen(getter, js_name = "mainCols")]
    pub fn main_cols(&self) -> usize {
        self.main_cols
    }
    #[wasm_bindgen(getter, js_name = "mainTimes")]
    pub fn main_times(&self) -> Float64Array {
        f64_array(&self.main_times[..])
    }
    #[wasm_bindgen(getter, js_name = "mainFrames")]
    pub fn main_frames(&self) -> Float64Array {
        f64_array(&self.main_frames[..])
    }

    // ── Slow frames ──────────────────────────────────────────────────
    #[wasm_bindgen(getter, js_name = "slowFieldNames")]
    pub fn slow_field_names(&self) -> Vec<String> {
        self.slow_field_names.clone()
    }
    #[wasm_bindgen(getter, js_name = "slowFieldSigned")]
    pub fn slow_field_signed(&self) -> Vec<u8> {
        self.slow_field_signed.clone()
    }
    #[wasm_bindgen(getter, js_name = "slowFieldUnits")]
    pub fn slow_field_units(&self) -> Vec<String> {
        self.slow_field_units.clone()
    }
    #[wasm_bindgen(getter, js_name = "slowCols")]
    pub fn slow_cols(&self) -> usize {
        self.slow_cols
    }
    #[wasm_bindgen(getter, js_name = "slowTimes")]
    pub fn slow_times(&self) -> Float64Array {
        f64_array(&self.slow_times[..])
    }
    #[wasm_bindgen(getter, js_name = "slowFrames")]
    pub fn slow_frames(&self) -> Float64Array {
        f64_array(&self.slow_frames[..])
    }

    // ── GPS frames ───────────────────────────────────────────────────
    // Empty arrays when no GPS — simpler for the JS side than dealing
    // with `null`. Check `hasGps` to know whether to use them.
    #[wasm_bindgen(getter, js_name = "gpsFieldNames")]
    pub fn gps_field_names(&self) -> Vec<String> {
        self.gps_field_names.clone()
    }
    #[wasm_bindgen(getter, js_name = "gpsFieldSigned")]
    pub fn gps_field_signed(&self) -> Vec<u8> {
        self.gps_field_signed.clone()
    }
    #[wasm_bindgen(getter, js_name = "gpsFieldUnits")]
    pub fn gps_field_units(&self) -> Vec<String> {
        self.gps_field_units.clone()
    }
    #[wasm_bindgen(getter, js_name = "gpsCols")]
    pub fn gps_cols(&self) -> usize {
        self.gps_cols
    }
    #[wasm_bindgen(getter, js_name = "gpsTimes")]
    pub fn gps_times(&self) -> Float64Array {
        f64_array(&self.gps_times[..])
    }
    #[wasm_bindgen(getter, js_name = "gpsFrames")]
    pub fn gps_frames(&self) -> Float64Array {
        f64_array(&self.gps_frames[..])
    }
}

/// Parse a blackbox log buffer.
///
/// `bytes` is the raw contents of a `.bbl` / `.bfl` / `.txt` file.
/// `main_stride` downsamples main frames at decode time — pass `1` to
/// emit every frame, higher values to thin the data set proportionally.
/// (GPS and slow frames are always emitted at full rate; they're already
/// 1–2 orders of magnitude rarer than main frames.)
///
/// Returns the first log inside the buffer; multi-log files (rare —
/// disarm + re-arm with the same SD card) drop subsequent logs for now.
#[wasm_bindgen(js_name = parseBlackbox)]
pub fn parse_blackbox(bytes: &[u8], main_stride: u32) -> Result<FlightLog, JsError> {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();

    let file = blackbox_log::File::new(bytes);
    if file.log_count() == 0 {
        return Err(JsError::new(
            "No valid log found in buffer. Is this actually a blackbox file?",
        ));
    }
    let headers = file
        .parse(0)
        .ok_or_else(|| JsError::new("Failed to read first log's headers"))?
        .map_err(|e| JsError::new(&format!("Header parse error: {e:?}")))?;

    // Pull field metadata for each frame type — the same arrays are then
    // used by the data-parser loop below to interpret signed vs unsigned
    // raw values.
    let (main_field_names, main_field_signed, main_field_units) =
        unpack_fields(headers.main_frame_def());
    let main_cols = main_field_names.len();

    let (slow_field_names, slow_field_signed, slow_field_units) =
        unpack_fields(headers.slow_frame_def());
    let slow_cols = slow_field_names.len();

    let (gps_field_names, gps_field_signed, gps_field_units, has_gps) =
        match headers.gps_frame_def() {
            Some(def) => {
                let (n, s, u) = unpack_fields(def);
                (n, s, u, true)
            }
            None => (Vec::new(), Vec::new(), Vec::new(), false),
        };
    let gps_cols = gps_field_names.len();

    // Convert u8 signed flags back to bool for fast in-loop branching.
    let main_signed: Vec<bool> = main_field_signed.iter().map(|&b| b != 0).collect();
    let slow_signed: Vec<bool> = slow_field_signed.iter().map(|&b| b != 0).collect();
    let gps_signed: Vec<bool> = gps_field_signed.iter().map(|&b| b != 0).collect();

    let mut main_times: Vec<f64> = Vec::new();
    let mut main_frames: Vec<f64> = Vec::new();
    let mut slow_times: Vec<f64> = Vec::new();
    let mut slow_frames: Vec<f64> = Vec::new();
    let mut gps_times: Vec<f64> = Vec::new();
    let mut gps_frames: Vec<f64> = Vec::new();

    let stride = main_stride.max(1) as u64;
    let mut main_seen: u64 = 0;
    let mut last_main_time_us: u64 = 0;

    let mut parser = headers.data_parser();
    while let Some(event) = parser.next() {
        match event {
            ParserEvent::Main(frame) => {
                last_main_time_us = frame.time_raw();
                let keep = main_seen % stride == 0;
                main_seen += 1;
                if !keep {
                    continue;
                }
                main_times.push(last_main_time_us as f64);
                push_raw_values(&frame, &main_signed, &mut main_frames);
            }
            ParserEvent::Slow(frame) => {
                slow_times.push(last_main_time_us as f64);
                push_raw_values(&frame, &slow_signed, &mut slow_frames);
            }
            ParserEvent::Gps(frame) => {
                if has_gps {
                    gps_times.push(last_main_time_us as f64);
                    push_raw_values(&frame, &gps_signed, &mut gps_frames);
                }
            }
            // Events (arming, mode change, error markers) aren't surfaced
            // in v0.1. The viewer only needs continuous frame data for
            // charts and 3D path; events become tab-bar hints later.
            ParserEvent::Event(_) => {}
        }
    }

    Ok(FlightLog {
        firmware: format!("{:?}", headers.firmware()),
        craft_name: headers.craft_name().unwrap_or("").to_string(),
        board_info: headers.board_info().unwrap_or("").to_string(),
        has_gps,
        main_field_names,
        main_field_signed,
        main_field_units,
        main_cols,
        main_times,
        main_frames,
        slow_field_names,
        slow_field_signed,
        slow_field_units,
        slow_cols,
        slow_times,
        slow_frames,
        gps_field_names,
        gps_field_signed,
        gps_field_units,
        gps_cols,
        gps_times,
        gps_frames,
    })
}

/// Build a `Float64Array` from a Rust slice using the explicit
/// `new_with_length` + `copy_from` pattern. `copy_from` is documented as
/// a single bulk memcpy (vs `Float64Array::from(&[f64])`, which on some
/// js-sys versions can fall back to per-element JS conversion). For a
/// 5 MB log we're moving ~600k f64 across the boundary; the difference
/// between memcpy (~1 ms) and per-element (~tens of seconds) is the
/// difference between a usable tool and a frozen tab.
fn f64_array(slice: &[f64]) -> Float64Array {
    let arr = Float64Array::new_with_length(slice.len() as u32);
    arr.copy_from(slice);
    arr
}

/// Walk a `FrameDef` once and return parallel name/signed/unit arrays.
fn unpack_fields<'data, D: FrameDef<'data>>(def: &D) -> (Vec<String>, Vec<u8>, Vec<String>) {
    let mut names = Vec::new();
    let mut signed = Vec::new();
    let mut units = Vec::new();
    for fd in def.iter() {
        names.push(fd.name.to_string());
        signed.push(if fd.signed { 1 } else { 0 });
        units.push(format!("{:?}", fd.unit.into()));
    }
    (names, signed, units)
}

/// Append all post-predictor raw values from `frame` to the flat row-major
/// `out` buffer. `signed[i]` controls how the i-th raw `u32` is reinterpreted
/// before being widened to `f64` — signed columns get the `as i32 as f64`
/// path so negative values round-trip correctly. Missing fields land as NaN.
fn push_raw_values<F: Frame>(frame: &F, signed: &[bool], out: &mut Vec<f64>) {
    for i in 0..frame.len() {
        let v = match frame.get_raw(i) {
            Some(raw) => {
                if *signed.get(i).unwrap_or(&false) {
                    (raw as i32) as f64
                } else {
                    raw as f64
                }
            }
            None => f64::NAN,
        };
        out.push(v);
    }
}
