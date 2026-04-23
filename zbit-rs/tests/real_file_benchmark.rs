use std::time::{SystemTime, UNIX_EPOCH};

use zbit_rs::{compress_adaptive_to_file, decompress_file};

#[test]
fn adaptive_pack_real_file_roundtrip_and_size_guard() {
    let input_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../studies/algorithmsResearch.md");
    let input = std::fs::read(&input_path).expect("read algorithmsResearch.md");

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let pack_path = std::env::temp_dir().join(format!("zbit_real_file_{stamp}.zbpk"));

    let stats = compress_adaptive_to_file(&input, &pack_path).expect("compress adaptive pack");
    let decoded = decompress_file(&pack_path).expect("decompress adaptive pack");
    std::fs::remove_file(&pack_path).ok();

    assert_eq!(decoded, input);

    // Adaptive method should never be worse than raw-copy candidate chosen by the same logic.
    assert!(
        stats.compressed_size <= stats.raw_candidate_bytes,
        "compressed size {} should be <= raw candidate {}",
        stats.compressed_size,
        stats.raw_candidate_bytes
    );
}
