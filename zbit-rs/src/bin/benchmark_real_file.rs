// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::env;
use std::fs;
use std::time::{Instant, SystemTime};

use zbit_rs::{compress_adaptive_to_file, decompress_file};

fn format_timestamp_local(now: SystemTime) -> String {
    let datetime: chrono_like::DateTimeParts = now.into();
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        datetime.year, datetime.month, datetime.day, datetime.hour, datetime.minute, datetime.second
    )
}

mod chrono_like {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    #[derive(Debug, Clone, Copy)]
    pub struct DateTimeParts {
        pub year: i32,
        pub month: u32,
        pub day: u32,
        pub hour: u32,
        pub minute: u32,
        pub second: u32,
    }

    impl From<SystemTime> for DateTimeParts {
        fn from(value: SystemTime) -> Self {
            let dur = value
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::from_secs(0));
            let secs = dur.as_secs() as i64;

            /*
             * UTC-like formatting without external crates.
             * Sufficient for benchmark report timestamping.
             */
            let days = secs.div_euclid(86_400);
            let secs_of_day = secs.rem_euclid(86_400);

            let (year, month, day) = civil_from_days(days);
            let hour = (secs_of_day / 3600) as u32;
            let minute = ((secs_of_day % 3600) / 60) as u32;
            let second = (secs_of_day % 60) as u32;

            Self {
                year,
                month,
                day,
                hour,
                minute,
                second,
            }
        }
    }

    fn civil_from_days(z: i64) -> (i32, u32, u32) {
        let z = z + 719468;
        let era = if z >= 0 { z } else { z - 146096 } / 146097;
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = mp + if mp < 10 { 3 } else { -9 };
        let year = y + if m <= 2 { 1 } else { 0 };

        (year as i32, m as u32, d as u32)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input_path = env::args()
        .nth(1)
        .unwrap_or_else(|| "../papers/zbit-algorithmsResearch.md".to_string());
    let pack_path = env::args()
        .nth(2)
        .unwrap_or_else(|| "benchmark_algorithmsResearch.zbpk".to_string());
    let report_path = env::args()
        .nth(3)
        .unwrap_or_else(|| "benchmark_latest.txt".to_string());

    let input = fs::read(&input_path)?;

    let t0 = Instant::now();
    let stats = compress_adaptive_to_file(&input, &pack_path)?;
    let compression_s = t0.elapsed().as_secs_f64();

    let t1 = Instant::now();
    let output = decompress_file(&pack_path)?;
    let decompression_s = t1.elapsed().as_secs_f64();

    let output_valid = input == output;

    let ratio = if stats.original_size > 0 {
        stats.compressed_size as f64 / stats.original_size as f64
    } else {
        0.0
    };
    let savings_percent = (1.0 - ratio) * 100.0;

    let mib = stats.original_size as f64 / (1024.0 * 1024.0);
    let compression_mibs = if compression_s > 0.0 { mib / compression_s } else { 0.0 };
    let decompression_mibs = if decompression_s > 0.0 { mib / decompression_s } else { 0.0 };

    let now = format_timestamp_local(SystemTime::now());

    let report = format!(
        "zBit-rs Real File Benchmark Report\n\
Generated: {now}\n\
Input file: {input_path}\n\
Compressed artifact: {pack_path}\n\
\n\
Selected method: {method}\n\
Selection reason: {reason}\n\
Circuit evaluation rule: {rule}\n\
\n\
Raw candidate size (bytes): {raw_candidate}\n\
Indexed-raw candidate size (bytes): {indexed_raw_candidate}\n\
Indexed-circuit candidate size (bytes): {indexed_circuit}\n\
\n\
Original size (bytes): {orig}\n\
Compressed size (bytes): {comp}\n\
Compression ratio (compressed/original): {ratio:.6}\n\
Space savings (%): {savings:.2}\n\
\n\
Compression time (ms): {comp_ms:.3}\n\
Decompression time (ms): {decomp_ms:.3}\n\
Compression throughput (MiB/s): {comp_mibs:.3}\n\
Decompression throughput (MiB/s): {decomp_mibs:.3}\n\
\n\
Unique symbols: {unique}\n\
Bits per symbol index: {bits}\n\
Raw dictionary bytes: {raw_dict}\n\
Circuit dictionary bytes: {circuit_dict}\n\
Packed index payload bytes: {payload}\n\
\n\
Output validation: {valid}\n",
        method = stats.chosen_method.name(),
        reason = stats.chosen_reason,
        rule = stats.circuit_rule_note,
        raw_candidate = stats.raw_candidate_bytes,
        indexed_raw_candidate = stats.indexed_raw_candidate_bytes,
        indexed_circuit = stats
            .indexed_circuit_candidate_bytes
            .map(|v| v.to_string())
            .unwrap_or_else(|| "skipped by rules".to_string()),
        orig = stats.original_size,
        comp = stats.compressed_size,
        ratio = ratio,
        savings = savings_percent,
        comp_ms = compression_s * 1000.0,
        decomp_ms = decompression_s * 1000.0,
        comp_mibs = compression_mibs,
        decomp_mibs = decompression_mibs,
        unique = stats.unique_symbols,
        bits = stats.bits_per_symbol,
        raw_dict = stats.raw_dictionary_bytes,
        circuit_dict = stats.circuit_dictionary_bytes,
        payload = stats.payload_bytes,
        valid = if output_valid { "PASS" } else { "FAIL" },
    );

    fs::write(&report_path, report.as_bytes())?;

    println!("benchmark input: {input_path}");
    println!("compressed artifact: {pack_path}");
    println!("report file: {report_path}");
    println!("selected method: {}", stats.chosen_method.name());
    println!("selection reason: {}", stats.chosen_reason);
    println!("original bytes: {}", stats.original_size);
    println!("compressed bytes: {}", stats.compressed_size);
    println!("output validation: {}", if output_valid { "PASS" } else { "FAIL" });

    if !output_valid {
        return Err("decompressed output does not match input".into());
    }

    Ok(())
}
