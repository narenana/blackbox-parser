//! Cross-log unit-sanity check.
//!
//! Walks several real iNAV blackbox logs and prints a markdown table of
//! the mid-flight values for the fields we're least sure about
//! (attitude, vbat, amperage, BaroAlt, GPS coord/altitude/speed/heading).
//! Both raw post-predictor and post-scaling values are shown so you can
//! eyeball whether the scaling factors are right.
//!
//! Usage:
//!   $env:BLACKBOX_TEST_FOLDER="C:/Users/Guddu/Desktop/Dolphin logs"
//!   cargo test unit_check --release -- --nocapture
//!
//! No log files are committed — the folder path is supplied at runtime.

use blackbox_log::frame::{Frame, FrameDef};
use blackbox_log::prelude::*;

const MIN_BYTES: u64 = 1_000_000; // skip tiny files (likely bench tests)

#[test]
fn unit_check_table() {
    let folder = match std::env::var("BLACKBOX_TEST_FOLDER") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Set BLACKBOX_TEST_FOLDER=<dir> to run.");
            return;
        }
    };

    let entries: Vec<_> = std::fs::read_dir(&folder)
        .expect("read dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.ends_with(".txt") || n.ends_with(".bbl") || n.ends_with(".bfl")
        })
        .filter(|e| {
            e.metadata()
                .map(|m| m.len() >= MIN_BYTES)
                .unwrap_or(false)
        })
        .collect();

    println!("\nFound {} logs ≥ {} bytes\n", entries.len(), MIN_BYTES);

    // Two markdown tables: per-log header summary, then per-log mid-flight values.
    println!("## Header summary\n");
    println!("| File | Size MB | Firmware | Craft | Main frames | GPS? | Duration s |");
    println!("|---|---|---|---|---|---|---|");

    let mut samples = Vec::new();
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let size_mb = entry.metadata().unwrap().len() as f64 / 1_048_576.0;

        let bytes = std::fs::read(&path).unwrap();
        let file = blackbox_log::File::new(&bytes);
        if file.log_count() == 0 {
            println!("| {name} | {size_mb:.1} | — | — | — | — | — |");
            continue;
        }
        let headers = match file.parse(0) {
            Some(Ok(h)) => h,
            _ => {
                println!("| {name} | {size_mb:.1} | parse error | | | | |");
                continue;
            }
        };

        let firmware = format!("{:?}", headers.firmware()).replace('|', "/");
        let craft = headers.craft_name().unwrap_or("").replace('|', "/");
        let has_gps = headers.gps_frame_def().is_some();

        // First pass: count frames + capture mid-flight indices.
        let mut main_count = 0usize;
        let mut gps_count = 0usize;
        let mut last_t_us = 0u64;
        let mut parser = headers.data_parser();
        while let Some(event) = parser.next() {
            match event {
                ParserEvent::Main(f) => {
                    last_t_us = f.time_raw();
                    main_count += 1;
                }
                ParserEvent::Gps(_) => gps_count += 1,
                _ => {}
            }
        }
        let duration_s = last_t_us as f64 / 1e6;

        println!(
            "| {name} | {size_mb:.1} | {firmware} | {craft} | {main_count} | {has_gps} | {duration_s:.1} |"
        );

        if main_count == 0 {
            continue;
        }

        // Second pass: walk to mid-flight and snapshot.
        let target_idx = main_count / 2;
        let mid = sample_mid_flight(&bytes, target_idx);
        if let Some(s) = mid {
            samples.push((name, s));
        }
    }

    if samples.is_empty() {
        println!("\nNo mid-flight samples gathered.");
        return;
    }

    // Build field schemas from the first log so we can print column headers.
    println!("\n## Mid-flight snapshot — raw values\n");
    println!("| File | t s | attitude[0..2] (raw) | vbat | amperage | BaroAlt | rssi | GPS lat,lon (raw) | GPS_alt | GPS_speed | GPS_hdg |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|");
    for (name, s) in &samples {
        println!(
            "| {name} | {:.1} | {} {} {} | {} | {} | {} | {} | {} | {} | {} | {} |",
            s.t_s,
            f(s.att0_raw),
            f(s.att1_raw),
            f(s.att2_raw),
            f(s.vbat_raw),
            f(s.amp_raw),
            f(s.baro_raw),
            f(s.rssi_raw),
            opt(s.gps_lat_raw),
            opt(s.gps_lon_raw),
            opt(s.gps_alt_raw),
            opt(s.gps_speed_raw),
        );
    }

    println!("\n## Mid-flight snapshot — after candidate scaling\n");
    println!("| File | roll° | pitch° | yaw° | vbat V (÷100) | amp A (÷100) | BaroAlt m (÷100) | RSSI raw | lat | lon | GPS_alt m (raw, iNAV 8 = m MSL) | GPS_speed km/h (×0.036) | GPS_hdg ° (÷10) |");
    println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
    for (name, s) in &samples {
        let lat = s.gps_lat_raw.map(|x| x as f64 / 1e7);
        let lon = s.gps_lon_raw.map(|x| x as f64 / 1e7);
        println!(
            "| {name} | {:.1} | {:.1} | {:.1} | {:.2} | {:.2} | {:.2} | {} | {} | {} | {} | {} | {} |",
            s.att0_raw as f64 / 10.0,
            s.att1_raw as f64 / 10.0,
            s.att2_raw as f64 / 10.0,
            s.vbat_raw as f64 / 100.0,
            s.amp_raw as f64 / 100.0,
            s.baro_raw as f64 / 100.0,
            s.rssi_raw,
            lat.map(|x| format!("{x:.5}")).unwrap_or_else(|| "—".into()),
            lon.map(|x| format!("{x:.5}")).unwrap_or_else(|| "—".into()),
            // iNAV 8 stores GPS altitude in METRES MSL — see comment in
            // edgetx-viewer/src/utils/parseBlackbox.js. No division.
            s.gps_alt_raw
                .map(|x| format!("{x}"))
                .unwrap_or_else(|| "—".into()),
            s.gps_speed_raw
                .map(|x| format!("{:.2}", x as f64 * 0.036))
                .unwrap_or_else(|| "—".into()),
            s.gps_hdg_raw
                .map(|x| format!("{:.1}", x as f64 / 10.0))
                .unwrap_or_else(|| "—".into()),
        );
    }

    println!();
}

#[derive(Default)]
struct MidSnap {
    t_s: f64,
    att0_raw: i64,
    att1_raw: i64,
    att2_raw: i64,
    vbat_raw: i64,
    amp_raw: i64,
    baro_raw: i64,
    rssi_raw: i64,
    gps_lat_raw: Option<i64>,
    gps_lon_raw: Option<i64>,
    gps_alt_raw: Option<i64>,
    gps_speed_raw: Option<i64>,
    gps_hdg_raw: Option<i64>,
}

fn sample_mid_flight(bytes: &[u8], target_idx: usize) -> Option<MidSnap> {
    let file = blackbox_log::File::new(bytes);
    let headers = file.parse(0)?.ok()?;

    // FrameDef is implemented on the owned types; the methods on Headers
    // already return references, so passing them straight through (no
    // extra `&`) gives the right `&D` shape these helpers want.
    let main_idx = field_index(headers.main_frame_def());
    let main_signed = field_signed(headers.main_frame_def());

    let gps_idx = headers.gps_frame_def().map(field_index);
    let gps_signed = headers.gps_frame_def().map(field_signed);

    let mut last_gps_lat = None;
    let mut last_gps_lon = None;
    let mut last_gps_alt = None;
    let mut last_gps_speed = None;
    let mut last_gps_hdg = None;

    let mut count = 0usize;
    let mut parser = headers.data_parser();
    while let Some(event) = parser.next() {
        match event {
            ParserEvent::Gps(f) => {
                if let (Some(idx), Some(sg)) = (gps_idx.as_ref(), gps_signed.as_ref()) {
                    last_gps_lat = idx.get("GPS_coord[0]").and_then(|&i| read_raw(&f, i, sg));
                    last_gps_lon = idx.get("GPS_coord[1]").and_then(|&i| read_raw(&f, i, sg));
                    last_gps_alt = idx.get("GPS_altitude").and_then(|&i| read_raw(&f, i, sg));
                    last_gps_speed = idx.get("GPS_speed").and_then(|&i| read_raw(&f, i, sg));
                    last_gps_hdg = idx.get("GPS_ground_course").and_then(|&i| read_raw(&f, i, sg));
                }
            }
            ParserEvent::Main(f) => {
                count += 1;
                if count == target_idx {
                    let mut s = MidSnap {
                        t_s: f.time_raw() as f64 / 1e6,
                        ..Default::default()
                    };
                    if let Some(&i) = main_idx.get("attitude[0]") {
                        s.att0_raw = read_raw(&f, i, &main_signed).unwrap_or(0);
                    }
                    if let Some(&i) = main_idx.get("attitude[1]") {
                        s.att1_raw = read_raw(&f, i, &main_signed).unwrap_or(0);
                    }
                    if let Some(&i) = main_idx.get("attitude[2]") {
                        s.att2_raw = read_raw(&f, i, &main_signed).unwrap_or(0);
                    }
                    if let Some(&i) = main_idx.get("vbat") {
                        s.vbat_raw = read_raw(&f, i, &main_signed).unwrap_or(0);
                    }
                    if let Some(&i) = main_idx.get("amperage") {
                        s.amp_raw = read_raw(&f, i, &main_signed).unwrap_or(0);
                    }
                    if let Some(&i) = main_idx.get("BaroAlt") {
                        s.baro_raw = read_raw(&f, i, &main_signed).unwrap_or(0);
                    }
                    if let Some(&i) = main_idx.get("rssi") {
                        s.rssi_raw = read_raw(&f, i, &main_signed).unwrap_or(0);
                    }
                    s.gps_lat_raw = last_gps_lat;
                    s.gps_lon_raw = last_gps_lon;
                    s.gps_alt_raw = last_gps_alt;
                    s.gps_speed_raw = last_gps_speed;
                    s.gps_hdg_raw = last_gps_hdg;
                    return Some(s);
                }
            }
            _ => {}
        }
    }
    None
}

fn field_index<'a, D: FrameDef<'a>>(def: &D) -> std::collections::HashMap<String, usize> {
    let mut out = std::collections::HashMap::new();
    for (i, fd) in def.iter().enumerate() {
        out.insert(fd.name.to_string(), i);
    }
    out
}

fn field_signed<'a, D: FrameDef<'a>>(def: &D) -> Vec<bool> {
    def.iter().map(|fd| fd.signed).collect()
}

fn read_raw<F: Frame>(frame: &F, i: usize, signed: &[bool]) -> Option<i64> {
    let raw = frame.get_raw(i)?;
    if *signed.get(i).unwrap_or(&false) {
        Some(raw as i32 as i64)
    } else {
        Some(raw as i64)
    }
}

fn f(v: i64) -> String {
    format!("{v}")
}
fn opt(v: Option<i64>) -> String {
    v.map(|x| x.to_string()).unwrap_or_else(|| "—".into())
}
