//! Quick sanity check that we can parse a real iNAV blackbox log end to
//! end. Runs only when the user points us at their log folder via the
//! `BLACKBOX_TEST_LOG` env var, so this test is opt-in and never fails on
//! a fresh checkout.
//!
//! Usage:
//!   $env:BLACKBOX_TEST_LOG="C:/Users/Guddu/Desktop/Dolphin logs/LOG00001.TXT"
//!   cargo test smoke -- --nocapture
//!
//! No log paths are committed — this exists purely as a scratchpad for
//! validating the wrapper against real-world data without bringing up the
//! whole WASM/JS pipeline.

use blackbox_log::frame::FrameDef;
use blackbox_log::prelude::*;

#[test]
fn smoke_parse_real_log() {
    let path = match std::env::var("BLACKBOX_TEST_LOG") {
        Ok(p) => p,
        Err(_) => {
            eprintln!("Set BLACKBOX_TEST_LOG=<path-to-bbl> to run this smoke test.");
            return;
        }
    };

    let bytes = std::fs::read(&path).expect("read log file");
    let file = blackbox_log::File::new(&bytes);

    println!("\n=== {path} ===");
    println!("Bytes: {}", bytes.len());
    println!("Log count: {}", file.log_count());

    assert!(file.log_count() > 0, "expected at least one log");

    let headers = file
        .parse(0)
        .expect("first log present")
        .expect("headers parse cleanly");

    println!("Firmware:   {:?}", headers.firmware());
    println!("Craft:      {:?}", headers.craft_name());
    println!("Board:      {:?}", headers.board_info());

    println!("Main fields ({}):", headers.main_frame_def().len());
    for fd in headers.main_frame_def().iter() {
        println!("  {:>22}  signed={}  unit={:?}", fd.name, fd.signed, fd.unit);
    }

    println!("Slow fields ({}):", headers.slow_frame_def().len());
    for fd in headers.slow_frame_def().iter() {
        println!("  {:>20}  signed={}  unit={:?}", fd.name, fd.signed, fd.unit);
    }

    match headers.gps_frame_def() {
        Some(def) => {
            println!("GPS fields ({}):", def.len());
            for fd in def.iter() {
                println!("  {:>20}  signed={}  unit={:?}", fd.name, fd.signed, fd.unit);
            }
        }
        None => println!("GPS: not present"),
    }

    // Drain the data parser. Counts frame types so we know the parser
    // actually walked the whole file.
    let mut parser = headers.data_parser();
    let mut main_count = 0usize;
    let mut slow_count = 0usize;
    let mut gps_count = 0usize;
    let mut event_count = 0usize;
    let mut last_t_us: u64 = 0;

    while let Some(event) = parser.next() {
        match event {
            ParserEvent::Main(f) => {
                main_count += 1;
                last_t_us = f.time_raw();
            }
            ParserEvent::Slow(_) => slow_count += 1,
            ParserEvent::Gps(_) => gps_count += 1,
            ParserEvent::Event(_) => event_count += 1,
        }
    }

    println!("Main frames:  {main_count}");
    println!("Slow frames:  {slow_count}");
    println!("GPS frames:   {gps_count}");
    println!("Events:       {event_count}");
    println!("Duration:     {:.2}s", last_t_us as f64 / 1e6);

    assert!(main_count > 0, "expected at least one main frame");
}
