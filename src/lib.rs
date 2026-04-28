//! `@narenana/blackbox-parser` — thin WASM facade over the `blackbox-log`
//! Rust crate. Designed to be dropped into any browser-side project that
//! needs to read Betaflight / iNAV / Cleanflight blackbox binary logs.
//!
//! ## v0.1 scope
//!
//! Eager parsing: read the entire byte buffer, walk every frame, return a
//! single `FlightLog` struct serialized to a JS object via
//! `serde-wasm-bindgen`. This is the simplest possible API and gets us
//! enough to render charts + a 3D path in the EdgeTX viewer.
//!
//! Trade-off: peak memory ~30–60 MB for a 5-minute 4 kHz log. Acceptable
//! for desktop browsers; might bite us on phones with ~1 minute logs at
//! 8 kHz. v0.2 will add a streaming iterator API once we hit the wall.
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
use serde::Serialize;
use wasm_bindgen::prelude::*;

// We deliberately avoid matching on `blackbox_log::Value` here. The crate
// re-exports a rich typed-value enum (Voltage, Velocity, Time, etc. — uom
// quantities), but for visualization we only need numbers, and consumers
// know from the field name what unit to expect. Going through the
// per-frame `get_raw()` API (which returns the post-predictor `u32`)
// keeps the wrapper independent of the upstream enum's exact variant set
// and lets us work with whatever firmware logs in the future.

/// JS-side representation of a parsed blackbox log. Serialized as a plain
/// JS object via `serde-wasm-bindgen`; consumers see it as
/// `{ firmware, craftName, mainFields, mainFrames, ... }`.
///
/// Field values are flattened to `f64` regardless of the underlying
/// `blackbox-log::Value` variant. We lose nanosecond precision on `Time`
/// (which is uom-wrapped), but for visualization that's fine — chart axes
/// don't need sub-microsecond resolution.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct FlightLog {
    /// Firmware identifier, e.g. "Betaflight 4.4.3" or "INAV 7.1".
    firmware: String,

    /// Optional craft name set by the pilot in the FC config.
    craft_name: String,

    /// Board identifier (FC target / hardware).
    board_info: String,

    /// Field definitions for main frames (gyro, motor, RC, etc.).
    main_fields: Vec<FieldDef>,

    /// Field definitions for slow frames (flight modes, state flags).
    slow_fields: Vec<FieldDef>,

    /// Field definitions for GPS frames. `None` if the FC had no GPS.
    gps_fields: Option<Vec<FieldDef>>,

    /// Main frame values. Outer index = frame number, inner index = field
    /// index aligned with `main_fields`.
    main_frames: Vec<Vec<f64>>,

    /// Microsecond timestamps, one per main frame.
    main_times: Vec<f64>,

    /// Slow frame values. Independent timeline from main frames; align via
    /// the timestamp column if needed for visualization.
    slow_frames: Vec<Vec<f64>>,

    /// GPS frame values. `None` if `gps_fields` is `None`.
    gps_frames: Option<Vec<Vec<f64>>>,
}

/// Per-field metadata. Mirrors `blackbox_log::frame::FieldDef` but with
/// owned strings so it crosses the FFI boundary cleanly.
#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct FieldDef {
    name: String,
    /// Unit identifier as a string (e.g. "deg/s", "amperage", "boolean").
    unit: String,
    signed: bool,
}

/// Parse a blackbox log buffer.
///
/// `bytes` is the raw contents of a `.bbl` / `.bfl` / `.txt` file (the
/// Betaflight TXT format from the early days is also supported by the
/// underlying crate). Returns the first log inside the buffer; multi-log
/// files (rare — happens when a pilot disarms and re-arms with the same
/// SD card) drop subsequent logs for now.
///
/// # Errors
///
/// Returns a `JsError` if the buffer doesn't contain a valid log header,
/// or if frame parsing produces a hard error mid-stream. Recoverable
/// per-frame errors from `blackbox-log` (corrupted I-frame mid-flight,
/// for instance) are silently skipped — the parser jumps to the next
/// frame and keeps going, matching the JS reference viewer's behaviour.
#[wasm_bindgen(js_name = parseBlackbox)]
pub fn parse_blackbox(bytes: &[u8]) -> Result<JsValue, JsError> {
    // Routes Rust panics to console.error with a stack trace.
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

    // Collect field defs and signedness in one pass so collect_raw_values
    // can use them later without re-iterating the def trees.
    let main_fields = collect_field_defs(headers.main_frame_def());
    let slow_fields = collect_field_defs(headers.slow_frame_def());
    // `gps_frame_def()` returns `Option<&GpsFrameDef>` directly (no Option-of-
    // owned-value), so `.as_ref()` would over-borrow into `Option<&&...>`,
    // which fails the `&D where D: FrameDef` bound on `collect_field_defs`.
    let gps_fields = headers.gps_frame_def().map(collect_field_defs);

    let main_signed: Vec<bool> = main_fields.iter().map(|f| f.signed).collect();
    let slow_signed: Vec<bool> = slow_fields.iter().map(|f| f.signed).collect();
    let gps_signed: Option<Vec<bool>> =
        gps_fields.as_ref().map(|f| f.iter().map(|f| f.signed).collect());

    let has_gps = gps_fields.is_some();

    let mut log = FlightLog {
        firmware: format!("{:?}", headers.firmware()),
        craft_name: headers.craft_name().unwrap_or("").to_string(),
        board_info: headers.board_info().unwrap_or("").to_string(),
        main_fields,
        slow_fields,
        gps_fields,
        gps_frames: if has_gps { Some(Vec::new()) } else { None },
        ..Default::default()
    };

    let mut parser = headers.data_parser();
    while let Some(event) = parser.next() {
        match event {
            ParserEvent::Main(frame) => {
                log.main_times.push(frame.time_raw() as f64);
                log.main_frames.push(collect_raw_values(&frame, &main_signed));
            }
            ParserEvent::Slow(frame) => {
                log.slow_frames.push(collect_raw_values(&frame, &slow_signed));
            }
            ParserEvent::Gps(frame) => {
                if let (Some(ref mut g), Some(ref signed)) = (&mut log.gps_frames, &gps_signed) {
                    g.push(collect_raw_values(&frame, signed));
                }
            }
            // Events (arming, mode change, error markers) aren't surfaced
            // in v0.1. The viewer only needs continuous frame data for
            // charts and 3D path; events become tab-bar hints later.
            ParserEvent::Event(_) => {}
        }
    }

    serde_wasm_bindgen::to_value(&log)
        .map_err(|e| JsError::new(&format!("Serialization error: {e}")))
}

/// Collect field definitions from a `FrameDef` reference into owned
/// `FieldDef`s. The trait is implemented on the owned type, so we take
/// `&D` here (`headers.{main,slow,gps}_frame_def()` all return refs).
fn collect_field_defs<'data, D: FrameDef<'data>>(def: &D) -> Vec<FieldDef> {
    def.iter()
        .map(|fd| FieldDef {
            name: fd.name.to_string(),
            unit: format!("{:?}", fd.unit.into()),
            signed: fd.signed,
        })
        .collect()
}

/// Read all post-predictor raw values from a parsed frame as `f64`. The
/// `signed` slice tells us which fields to interpret as `i32` rather than
/// `u32`. Consumers do unit conversion downstream based on the field
/// name (e.g. `gpsLatLon` → degrees ÷ 1e7, `gpsAltitude` → metres ÷ 100).
/// This keeps us decoupled from `blackbox_log::Value`'s variant set.
fn collect_raw_values<F: Frame>(frame: &F, signed: &[bool]) -> Vec<f64> {
    (0..frame.len())
        .map(|i| match frame.get_raw(i) {
            Some(raw) => {
                if *signed.get(i).unwrap_or(&false) {
                    (raw as i32) as f64
                } else {
                    raw as f64
                }
            }
            None => f64::NAN,
        })
        .collect()
}
