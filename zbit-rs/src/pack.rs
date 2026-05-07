// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap, HashSet};
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::time::Instant;

use crate::error::{ZbitError, ZbitResult};
use crate::model::ZbitModel;
use crate::pack_rules::{choose_best_method, should_evaluate_circuit, PackEvaluation, PackMethod};
use blake3::Hasher as Blake3Hasher;
use crc32fast::Hasher as Crc32Hasher;
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use preflate_rs::{preflate_whole_deflate_stream, recreate_whole_deflate_stream, PreflateConfig};
use rayon::prelude::*;
use xxhash_rust::xxh3::xxh3_64;
use xz2::read::XzDecoder;
use xz2::stream::{Check, Filters, LzmaOptions, MatchFinder, Mode, Stream};
use xz2::write::XzEncoder as XzWriterEncoder;
use zstd::stream as zstd_stream;

pub const ZBPK_MAGIC: u32 = 0x5A42_504B; // "ZBPK"
pub const ZBPK_VERSION: u16 = 2;
pub const ZBPK_HEADER_BYTES: usize = 36;
pub const ZBPS_MAGIC: u32 = 0x5A42_5053; // "ZBPS"
pub const ZBPS_VERSION: u16 = 1;
const ZBPS_HEADER_BYTES: usize = 40;
const ZBPS_BLOCK_HEADER_BYTES: usize = 21;
const ZBPS_NODE_KIND_PIECE: u8 = 0;
const ZBPS_NODE_KIND_GROUP: u8 = 1;
const ZBPS_NODE_KIND_SPLIT: u8 = 2;
const ZBPS_NODE_KIND_GLOBAL_SLICE: u8 = 3;
const ZBPS_HISTORY_NONE: u8 = u8::MAX;
const ZBPS_FLAG_CARRY_GROUPING_HISTORY: u16 = 0x0001;
const ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS: u16 = 0x0002;
const ZBPS_FLAG_SHARED_GROUPING_PAYLOAD: u16 = 0x0004;
const MAX_HUFFMAN_CODE_BITS: u8 = 56;
const ZBPK_MAX_OUTPUT_BYTES: usize = 1usize << 30; // 1 GiB hard safety bound

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum CompressionProfile {
    Fast,
    Balanced,
    Deep,
    Research,
}

impl CompressionProfile {
    fn from_env() -> Self {
        let value = std::env::var("ZBIT_COMPRESSION_PROFILE")
            .or_else(|_| std::env::var("ZBIT_PROFILE"))
            .unwrap_or_else(|_| "balanced".to_string());
        match value.trim().to_ascii_lowercase().as_str() {
            "fast" => Self::Fast,
            "deep" => Self::Deep,
            "research" => Self::Research,
            _ => Self::Balanced,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Deep => "deep",
            Self::Research => "research",
        }
    }

    fn max_transform_plans(self) -> usize {
        match self {
            Self::Fast => 4,
            Self::Balanced => 8,
            Self::Deep => 12,
            Self::Research => 16,
        }
    }

    fn correction_transform_plan_budget(self) -> usize {
        match self {
            Self::Fast => 4,
            Self::Balanced => 8,
            Self::Deep => 12,
            Self::Research => 16,
        }
    }

    fn enable_xz_extreme_for_raw_xz(self) -> bool {
        !matches!(self, Self::Fast)
    }

    fn enable_xz_extreme_refinement(self) -> bool {
        !matches!(self, Self::Fast)
    }

    fn should_attempt_recursive_on_realtime_blocks(self) -> bool {
        !matches!(self, Self::Fast)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CandidateTimingStats {
    pub index_stream_ms: f64,
    pub huffman_stream_ms: f64,
    pub raw_deflate_ms: f64,
    pub raw_zstd_ms: f64,
    pub raw_xz_ms: f64,
    pub framed_extraction_ms: f64,
    pub recursive_preflate_ms: f64,
    pub recursive_transform_sampling_ms: f64,
    pub recursive_transform_eval_ms: f64,
    pub recursive_correction_modeling_ms: f64,
    pub recursive_total_ms: f64,
    pub candidate_validation_ms: f64,
    pub stream_block_planning_ms: f64,
    pub stream_global_payload_ms: f64,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub codec_hits: usize,
    pub codec_misses: usize,
    pub preflate_hits: usize,
    pub preflate_misses: usize,
    pub stream_range_hits: usize,
    pub stream_range_misses: usize,
}

#[derive(Debug, Clone)]
pub struct PackStats {
    pub original_size: usize,
    pub compressed_size: usize,

    pub unique_symbols: usize,
    pub bits_per_symbol: u8,
    pub payload_bytes: usize,

    pub raw_dictionary_bytes: usize,
    pub circuit_dictionary_bytes: usize,
    pub huffman_dictionary_bytes: usize,

    pub raw_candidate_bytes: usize,
    pub indexed_raw_candidate_bytes: usize,
    pub indexed_circuit_candidate_bytes: Option<usize>,
    pub indexed_huffman_candidate_bytes: Option<usize>,
    pub raw_deflate_candidate_bytes: Option<usize>,
    pub raw_zstd_candidate_bytes: Option<usize>,
    pub raw_xz_candidate_bytes: Option<usize>,
    pub framed_raw_candidate_bytes: Option<usize>,
    pub recursive_circuit_xz_candidate_bytes: Option<usize>,
    pub monotonic_delta_candidate_bytes: Option<usize>,

    pub chosen_method: PackMethod,
    pub chosen_reason: String,
    pub circuit_rule_note: String,
    pub active_profile: String,
    pub skipped_candidates: Vec<String>,
    pub timings: CandidateTimingStats,
    pub cache_stats: CacheStats,
}

#[derive(Debug, Clone)]
pub struct StreamPackOptions {
    pub chunk_size: usize,
    pub key_piece_interval: usize,
    pub max_group_depth: u8,
    pub max_group_pieces: usize,
    pub carry_grouping_history: bool,
    pub realtime_mode: bool,
    pub wide_overfitting_circuits: bool,
}

impl Default for StreamPackOptions {
    fn default() -> Self {
        Self {
            chunk_size: 256 * 1024,
            key_piece_interval: 8,
            max_group_depth: 2,
            max_group_pieces: 8,
            carry_grouping_history: true,
            realtime_mode: true,
            wide_overfitting_circuits: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamPackStats {
    pub original_size: usize,
    pub compressed_size: usize,
    pub chunk_size: usize,
    pub total_chunks: usize,
    pub key_piece_interval: usize,
    pub key_piece_count: usize,
    pub block_count: usize,
    pub max_group_depth: u8,
    pub max_group_pieces: usize,
    pub piece_node_count: usize,
    pub grouped_node_count: usize,
    pub split_node_count: usize,
    pub max_depth_used: u8,
    pub grouping_hint_updates: usize,
    pub key_piece_decode_note: String,
    pub effective_wide_overfitting_circuits: bool,
    pub adaptive_wide_promotion_used: bool,
    pub shared_grouping_payload_used: bool,
    pub active_profile: String,
    pub skipped_candidates: Vec<String>,
    pub timings: CandidateTimingStats,
    pub cache_stats: CacheStats,
}

#[derive(Debug, Clone)]
struct IndexStream {
    unique_symbols: Vec<u8>,
    bits_per_symbol: u8,
    payload: Vec<u8>,
    frequencies: [u32; 256],
}

#[derive(Debug, Clone)]
struct HuffmanStream {
    symbols: Vec<u8>,
    code_lengths: Vec<u8>,
    payload: Vec<u8>,
}

#[derive(Debug, Clone)]
struct HuffmanNode {
    symbol: Option<u8>,
    left: Option<usize>,
    right: Option<usize>,
}

#[derive(Debug, Clone)]
struct FramedPayloadRun {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    frame_tag: [u8; 4],
    payload: Vec<u8>,
    base_chunk_len: u32,
    full_chunk_count: u32,
    tail_chunk_len: u32,
    total_chunks: u32,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
enum CircuitTransformKind {
    Identity,
    DeltaPrev,
    XorPrev,
    BitPlaneTranspose,
    BitPlaneTransposeDelta,
    BitPlaneTransposeXor,
    PeriodicHeadTail,
    PeriodicGather,
    PeriodicDelta,
    PeriodicXor,
    PeriodicGatherDelta,
    PeriodicGatherXor,
    PeriodicHeadTailTailGather,
    PeriodicHeadTailTailGatherDelta,
    PeriodicHeadTailTailDelta,
    PeriodicHeadTailTailXor,
    PeriodicHeadTailDelta,
    PeriodicHeadTailXor,
}

impl CircuitTransformKind {
    fn name(self) -> &'static str {
        match self {
            Self::Identity => "identity",
            Self::DeltaPrev => "delta-prev",
            Self::XorPrev => "xor-prev",
            Self::BitPlaneTranspose => "bit-plane-transpose",
            Self::BitPlaneTransposeDelta => "bit-plane-transpose-delta",
            Self::BitPlaneTransposeXor => "bit-plane-transpose-xor",
            Self::PeriodicHeadTail => "periodic-head-tail",
            Self::PeriodicGather => "periodic-gather",
            Self::PeriodicDelta => "periodic-delta",
            Self::PeriodicXor => "periodic-xor",
            Self::PeriodicGatherDelta => "periodic-gather-delta",
            Self::PeriodicGatherXor => "periodic-gather-xor",
            Self::PeriodicHeadTailTailGather => "periodic-head-tail-tail-gather",
            Self::PeriodicHeadTailTailGatherDelta => "periodic-head-tail-tail-gather-delta",
            Self::PeriodicHeadTailTailDelta => "periodic-head-tail-tail-delta",
            Self::PeriodicHeadTailTailXor => "periodic-head-tail-tail-xor",
            Self::PeriodicHeadTailDelta => "periodic-head-tail-delta",
            Self::PeriodicHeadTailXor => "periodic-head-tail-xor",
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Identity => 0,
            Self::DeltaPrev => 1,
            Self::XorPrev => 2,
            Self::BitPlaneTranspose => 3,
            Self::BitPlaneTransposeDelta => 16,
            Self::BitPlaneTransposeXor => 17,
            Self::PeriodicHeadTail => 4,
            Self::PeriodicGather => 5,
            Self::PeriodicDelta => 6,
            Self::PeriodicXor => 7,
            Self::PeriodicGatherDelta => 8,
            Self::PeriodicGatherXor => 9,
            Self::PeriodicHeadTailTailGather => 10,
            Self::PeriodicHeadTailTailGatherDelta => 11,
            Self::PeriodicHeadTailTailDelta => 12,
            Self::PeriodicHeadTailTailXor => 13,
            Self::PeriodicHeadTailDelta => 14,
            Self::PeriodicHeadTailXor => 15,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Identity),
            1 => Some(Self::DeltaPrev),
            2 => Some(Self::XorPrev),
            3 => Some(Self::BitPlaneTranspose),
            16 => Some(Self::BitPlaneTransposeDelta),
            17 => Some(Self::BitPlaneTransposeXor),
            4 => Some(Self::PeriodicHeadTail),
            5 => Some(Self::PeriodicGather),
            6 => Some(Self::PeriodicDelta),
            7 => Some(Self::PeriodicXor),
            8 => Some(Self::PeriodicGatherDelta),
            9 => Some(Self::PeriodicGatherXor),
            10 => Some(Self::PeriodicHeadTailTailGather),
            11 => Some(Self::PeriodicHeadTailTailGatherDelta),
            12 => Some(Self::PeriodicHeadTailTailDelta),
            13 => Some(Self::PeriodicHeadTailTailXor),
            14 => Some(Self::PeriodicHeadTailDelta),
            15 => Some(Self::PeriodicHeadTailXor),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct CircuitTransformPlan {
    kind: CircuitTransformKind,
    period: u32,
    head: u32,
}

#[derive(Debug, Clone)]
struct CircuitTopologyNode {
    id: u32,
    parent_id: u32,
    relation: u8, // 0 = series, 1 = parallel
    order: u16,
    kind: u8,
    param_a: u32,
    param_b: u32,
    hash64: u64,
}

#[derive(Debug, Clone, Copy)]
enum PayloadCodec {
    Raw,
    Xz,
    Zstd,
    XzExtreme,
}

impl PayloadCodec {
    fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Xz => "xz",
            Self::Zstd => "zstd",
            Self::XzExtreme => "xz-extreme",
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Raw => 0,
            Self::Xz => 1,
            Self::Zstd => 2,
            Self::XzExtreme => 3,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Raw),
            1 => Some(Self::Xz),
            2 => Some(Self::Zstd),
            3 => Some(Self::XzExtreme),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct RecursiveCircuitStream {
    base: FramedPayloadRun,
    transformed_payload: Vec<u8>,
    corrections_payload: Vec<u8>,
    plain_len: usize,
    transformed_encoded_len: usize,
    correction_plain_len: usize,
    correction_encoded_len: usize,
    transformed_codec: PayloadCodec,
    correction_codec: PayloadCodec,
    zlib_header: [u8; 2],
    zlib_adler32: [u8; 4],
    transform_plan: CircuitTransformPlan,
    topology: Vec<CircuitTopologyNode>,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
enum MonotonicDeltaMode {
    GapVarint,
    GapDeltaVarint,
    GapBytes,
    GapTrailingZeroVarint,
    GapTrailingZeroBytes,
}

impl MonotonicDeltaMode {
    fn as_u8(self) -> u8 {
        match self {
            Self::GapVarint => 0,
            Self::GapDeltaVarint => 1,
            Self::GapBytes => 2,
            Self::GapTrailingZeroVarint => 3,
            Self::GapTrailingZeroBytes => 4,
        }
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::GapVarint),
            1 => Some(Self::GapDeltaVarint),
            2 => Some(Self::GapBytes),
            3 => Some(Self::GapTrailingZeroVarint),
            4 => Some(Self::GapTrailingZeroBytes),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
struct MonotonicDeltaStream {
    width: u8,
    count: u64,
    first_value: u64,
    transformed_plain_len: usize,
    mode: MonotonicDeltaMode,
    trailing_zero_shift: u8,
    codec: PayloadCodec,
    payload: Vec<u8>,
}

#[derive(Debug, Default)]
struct DecodeNode {
    symbol: Option<u8>,
    left: Option<Box<DecodeNode>>,
    right: Option<Box<DecodeNode>>,
}

#[derive(Debug, Clone, Copy)]
struct StreamHeader {
    flags: u16,
    chunk_size: usize,
    key_piece_interval: usize,
    original_size: usize,
    total_chunks: usize,
    block_count: usize,
}

#[derive(Debug, Clone)]
struct PackedRangeCandidate {
    pack_bytes: Vec<u8>,
    method: PackMethod,
    original_size: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct PayloadHash {
    fast64: u64,
    strong128: [u8; 16],
    len: usize,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct CodecProfileKey {
    allow_raw: bool,
    allow_xz_extreme: bool,
    profile: CompressionProfile,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
struct StreamRangeCacheKey {
    input_hash: PayloadHash,
    abs_start_chunk: usize,
    abs_end_chunk: usize,
    realtime_mode: bool,
    allow_recursive_candidate: bool,
    profile: CompressionProfile,
}

#[derive(Debug, Default)]
struct CompressionCache {
    codec_outputs: HashMap<(PayloadHash, CodecProfileKey), (PayloadCodec, Vec<u8>)>,
    preflate_outputs: HashMap<(PayloadHash, u32), Option<(Vec<u8>, Vec<u8>)>>,
    stream_range_candidates: HashMap<StreamRangeCacheKey, PackedRangeCandidate>,
}

#[derive(Debug)]
struct CompressionContext {
    profile: CompressionProfile,
    timings: CandidateTimingStats,
    skipped_candidates: Vec<String>,
    cache_stats: CacheStats,
    cache: CompressionCache,
}

impl CompressionContext {
    fn new(profile: CompressionProfile) -> Self {
        Self {
            profile,
            timings: CandidateTimingStats::default(),
            skipped_candidates: Vec::new(),
            cache_stats: CacheStats::default(),
            cache: CompressionCache::default(),
        }
    }

    fn push_skipped(&mut self, note: impl Into<String>) {
        self.skipped_candidates.push(note.into());
    }
}

#[derive(Debug, Clone)]
enum StreamNodeKind {
    Piece {
        chunk_len: usize,
        method: PackMethod,
        pack_bytes: Vec<u8>,
    },
    Group {
        chunk_count: usize,
        original_len: usize,
        method: PackMethod,
        pack_bytes: Vec<u8>,
    },
    Split {
        level: u8,
        left: Box<StreamNode>,
        right: Box<StreamNode>,
    },
    GlobalSlice {
        chunk_count: usize,
        original_offset: usize,
        original_len: usize,
    },
}

#[derive(Debug, Clone)]
struct StreamNode {
    kind: StreamNodeKind,
    chunk_count: usize,
    original_len: usize,
    encoded_size: usize,
}

#[derive(Debug, Clone)]
struct StreamDecodedNode {
    bytes: Vec<u8>,
    chunk_count: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct StreamNodeCounts {
    piece_nodes: usize,
    grouped_nodes: usize,
    split_nodes: usize,
    max_depth_used: u8,
}

fn bits_needed_for_count(count: usize) -> u8 {
    if count <= 1 {
        return 1;
    }

    let mut bits = 0u8;
    let mut value = count - 1;
    while value > 0 {
        bits += 1;
        value >>= 1;
    }
    bits
}

fn pack_symbol_index(payload: &mut [u8], bit_offset: usize, value: u32, bits: u8) {
    for i in 0..bits {
        if ((value >> i) & 1) != 0 {
            let pos = bit_offset + i as usize;
            payload[pos >> 3] |= 1u8 << (pos & 7);
        }
    }
}

fn unpack_symbol_index(payload: &[u8], bit_offset: usize, bits: u8) -> u32 {
    let mut value = 0u32;
    for i in 0..bits {
        let pos = bit_offset + i as usize;
        let bit = (payload[pos >> 3] >> (pos & 7)) & 1;
        value |= (bit as u32) << i;
    }
    value
}

fn bit_at_msb_first(payload: &[u8], bit_index: usize) -> Option<u8> {
    let byte = *payload.get(bit_index >> 3)?;
    let shift = 7usize.saturating_sub(bit_index & 7);
    Some((byte >> shift) & 1)
}

fn push_bit_msb_first(out: &mut Vec<u8>, bit_index: &mut usize, bit: bool) {
    if (*bit_index & 7) == 0 {
        out.push(0);
    }

    if bit {
        let byte_idx = *bit_index >> 3;
        let shift = 7usize.saturating_sub(*bit_index & 7);
        out[byte_idx] |= 1u8 << shift;
    }

    *bit_index += 1;
}

fn push_code_msb_first(out: &mut Vec<u8>, bit_index: &mut usize, code: u64, len: u8) {
    for shift in (0..(len as usize)).rev() {
        let bit = ((code >> shift) & 1) != 0;
        push_bit_msb_first(out, bit_index, bit);
    }
}

fn payload_hash(data: &[u8]) -> PayloadHash {
    let fast64 = xxh3_64(data);
    let mut hasher = Blake3Hasher::new();
    hasher.update(data);
    let digest = hasher.finalize();
    let mut strong128 = [0u8; 16];
    strong128.copy_from_slice(&digest.as_bytes()[..16]);
    PayloadHash {
        fast64,
        strong128,
        len: data.len(),
    }
}

fn build_index_stream(input: &[u8]) -> ZbitResult<IndexStream> {
    let mut present = [false; 256];
    let mut id_map = [u16::MAX; 256];
    let mut frequencies = [0u32; 256];

    for &b in input {
        present[b as usize] = true;
        frequencies[b as usize] = frequencies[b as usize].saturating_add(1);
    }

    let mut unique_symbols = Vec::with_capacity(256);
    for b in 0u16..=255 {
        if present[b as usize] {
            id_map[b as usize] = unique_symbols.len() as u16;
            unique_symbols.push(b as u8);
        }
    }

    let bits = bits_needed_for_count(unique_symbols.len());
    let payload_bits = input.len() * bits as usize;
    let payload_bytes = (payload_bits + 7) / 8;
    let mut payload = vec![0u8; payload_bytes];

    let mut bit_offset = 0usize;
    for &b in input {
        let id = id_map[b as usize];
        if id == u16::MAX {
            return Err(ZbitError::Internal(
                "id_map missing symbol while packing".to_string(),
            ));
        }
        pack_symbol_index(&mut payload, bit_offset, id as u32, bits);
        bit_offset += bits as usize;
    }

    Ok(IndexStream {
        unique_symbols,
        bits_per_symbol: bits,
        payload,
        frequencies,
    })
}

fn compute_huffman_lengths(symbols: &[u8], frequencies: &[u32; 256]) -> ZbitResult<Vec<u8>> {
    if symbols.is_empty() {
        return Ok(Vec::new());
    }

    if symbols.len() == 1 {
        return Ok(vec![1u8]);
    }

    let mut nodes = Vec::with_capacity(symbols.len() * 2);
    let mut heap = BinaryHeap::<Reverse<(u64, usize, usize)>>::new();
    let mut tie = 0usize;

    for &symbol in symbols {
        let freq = frequencies[symbol as usize] as u64;
        nodes.push(HuffmanNode {
            symbol: Some(symbol),
            left: None,
            right: None,
        });
        let idx = nodes.len() - 1;
        heap.push(Reverse((freq.max(1), tie, idx)));
        tie += 1;
    }

    while heap.len() > 1 {
        let Reverse((f_a, _, idx_a)) = heap
            .pop()
            .ok_or_else(|| ZbitError::Internal("huffman heap unexpectedly empty".to_string()))?;
        let Reverse((f_b, _, idx_b)) = heap
            .pop()
            .ok_or_else(|| ZbitError::Internal("huffman heap unexpectedly empty".to_string()))?;

        nodes.push(HuffmanNode {
            symbol: None,
            left: Some(idx_a),
            right: Some(idx_b),
        });

        let parent = nodes.len() - 1;
        heap.push(Reverse((f_a.saturating_add(f_b), tie, parent)));
        tie += 1;
    }

    let Reverse((_, _, root)) = heap
        .pop()
        .ok_or_else(|| ZbitError::Internal("huffman root missing".to_string()))?;

    let mut length_by_symbol = [0u8; 256];
    let mut stack = vec![(root, 0u8)];

    while let Some((node_idx, depth)) = stack.pop() {
        let node = nodes
            .get(node_idx)
            .ok_or_else(|| ZbitError::Internal("huffman node index out of range".to_string()))?;

        if let Some(symbol) = node.symbol {
            let assigned = depth.max(1);
            length_by_symbol[symbol as usize] = assigned;
            continue;
        }

        let next_depth = depth.saturating_add(1);
        if let Some(left) = node.left {
            stack.push((left, next_depth));
        }
        if let Some(right) = node.right {
            stack.push((right, next_depth));
        }
    }

    let lengths = symbols
        .iter()
        .map(|&symbol| length_by_symbol[symbol as usize])
        .collect::<Vec<_>>();

    Ok(lengths)
}

fn build_canonical_codes(symbols: &[u8], code_lengths: &[u8]) -> ZbitResult<Vec<(u8, u64, u8)>> {
    if symbols.len() != code_lengths.len() {
        return Err(ZbitError::Internal(
            "huffman symbols and lengths length mismatch".to_string(),
        ));
    }

    let mut entries = symbols
        .iter()
        .copied()
        .zip(code_lengths.iter().copied())
        .collect::<Vec<_>>();

    if entries.is_empty() {
        return Ok(Vec::new());
    }

    if entries.iter().any(|(_, len)| *len == 0) {
        return Err(ZbitError::Internal(
            "zero-length huffman code in non-empty dictionary".to_string(),
        ));
    }

    if entries.iter().any(|(_, len)| *len > MAX_HUFFMAN_CODE_BITS) {
        return Err(ZbitError::Limit(format!(
            "huffman code length exceeds {MAX_HUFFMAN_CODE_BITS} bits"
        )));
    }

    entries.sort_unstable_by_key(|(symbol, len)| (*len, *symbol));

    let mut out = Vec::with_capacity(entries.len());
    let mut code = 0u64;
    let mut prev_len = entries[0].1;

    out.push((entries[0].0, 0u64, prev_len));

    for (symbol, len) in entries.into_iter().skip(1) {
        if len < prev_len {
            return Err(ZbitError::Internal(
                "non-monotonic canonical length order".to_string(),
            ));
        }

        let shift = (len - prev_len) as u32;
        code = code
            .checked_add(1)
            .ok_or_else(|| ZbitError::Internal("canonical huffman code overflow".to_string()))?;
        code = code
            .checked_shl(shift)
            .ok_or_else(|| ZbitError::Internal("canonical huffman shift overflow".to_string()))?;

        out.push((symbol, code, len));
        prev_len = len;
    }

    Ok(out)
}

fn build_huffman_stream(input: &[u8], stream: &IndexStream) -> ZbitResult<Option<HuffmanStream>> {
    if input.is_empty() {
        return Ok(None);
    }

    let symbols = stream.unique_symbols.clone();
    let code_lengths = compute_huffman_lengths(&symbols, &stream.frequencies)?;

    if code_lengths.iter().any(|&len| len > MAX_HUFFMAN_CODE_BITS) {
        return Ok(None);
    }

    let codes = build_canonical_codes(&symbols, &code_lengths)?;

    let mut code_by_symbol = [0u64; 256];
    let mut len_by_symbol = [0u8; 256];
    for (symbol, code, len) in codes {
        code_by_symbol[symbol as usize] = code;
        len_by_symbol[symbol as usize] = len;
    }

    let mut payload = Vec::new();
    let mut bit_index = 0usize;
    for &byte in input {
        let len = len_by_symbol[byte as usize];
        if len == 0 {
            return Err(ZbitError::Internal(
                "missing huffman code for input symbol".to_string(),
            ));
        }
        let code = code_by_symbol[byte as usize];
        push_code_msb_first(&mut payload, &mut bit_index, code, len);
    }

    Ok(Some(HuffmanStream {
        symbols,
        code_lengths,
        payload,
    }))
}

fn decode_huffman_dictionary(
    dict_bytes: &[u8],
    unique_count: usize,
) -> ZbitResult<(Vec<u8>, Vec<u8>)> {
    if dict_bytes.len() != unique_count.saturating_mul(2) {
        return Err(ZbitError::Parse(
            "indexed-huffman dictionary size must be 2 * unique_count".to_string(),
        ));
    }

    let mut symbols = Vec::with_capacity(unique_count);
    let mut lengths = Vec::with_capacity(unique_count);

    for i in 0..unique_count {
        let symbol = dict_bytes[i * 2];
        let len = dict_bytes[i * 2 + 1];

        if len == 0 || len > MAX_HUFFMAN_CODE_BITS {
            return Err(ZbitError::Parse(
                "indexed-huffman dictionary contains invalid code length".to_string(),
            ));
        }

        symbols.push(symbol);
        lengths.push(len);
    }

    Ok((symbols, lengths))
}

fn insert_decode_code(root: &mut DecodeNode, code: u64, len: u8, symbol: u8) -> ZbitResult<()> {
    let mut cursor = root;

    for shift in (0..(len as usize)).rev() {
        let bit = ((code >> shift) & 1) as u8;

        if bit == 0 {
            let next = cursor
                .left
                .get_or_insert_with(|| Box::<DecodeNode>::default());
            cursor = next.as_mut();
        } else {
            let next = cursor
                .right
                .get_or_insert_with(|| Box::<DecodeNode>::default());
            cursor = next.as_mut();
        }
    }

    if cursor.symbol.replace(symbol).is_some() {
        return Err(ZbitError::Parse(
            "indexed-huffman dictionary assigns duplicate code".to_string(),
        ));
    }

    Ok(())
}

fn decode_huffman_payload(
    payload: &[u8],
    original_size: usize,
    symbols: &[u8],
    code_lengths: &[u8],
) -> ZbitResult<Vec<u8>> {
    let codes = build_canonical_codes(symbols, code_lengths)
        .map_err(|e| ZbitError::Parse(format!("invalid canonical huffman dictionary: {e}")))?;

    let mut root = DecodeNode::default();
    for (symbol, code, len) in codes {
        insert_decode_code(&mut root, code, len, symbol)?;
    }

    let mut out = vec![0u8; original_size];
    let mut bit_index = 0usize;

    for byte in out.iter_mut() {
        let mut node = &root;

        loop {
            if let Some(symbol) = node.symbol {
                *byte = symbol;
                break;
            }

            let bit = bit_at_msb_first(payload, bit_index).ok_or_else(|| {
                ZbitError::Parse("indexed-huffman payload ended before decoding output".to_string())
            })?;
            bit_index += 1;

            node = if bit == 0 {
                node.left
                    .as_deref()
                    .ok_or_else(|| ZbitError::Parse("invalid huffman prefix path".to_string()))?
            } else {
                node.right
                    .as_deref()
                    .ok_or_else(|| ZbitError::Parse("invalid huffman prefix path".to_string()))?
            };
        }
    }

    Ok(out)
}

fn build_raw_deflate_payload(input: &[u8]) -> ZbitResult<Vec<u8>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(input)
        .map_err(|e| ZbitError::Io(format!("zlib write failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| ZbitError::Io(format!("zlib finish failed: {e}")))
}

fn decode_raw_deflate_payload(payload: &[u8], original_size: usize) -> ZbitResult<Vec<u8>> {
    let mut decoder = ZlibDecoder::new(payload);
    let mut out = Vec::with_capacity(original_size.min(1 << 20));
    decoder
        .read_to_end(&mut out)
        .map_err(|e| ZbitError::Parse(format!("zlib decode failed: {e}")))?;

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "raw-deflate output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

fn build_raw_zstd_payload(input: &[u8]) -> ZbitResult<Vec<u8>> {
    zstd_stream::encode_all(input, 19)
        .map_err(|e| ZbitError::Io(format!("zstd encode failed: {e}")))
}

fn decode_raw_zstd_payload(payload: &[u8], original_size: usize) -> ZbitResult<Vec<u8>> {
    let out = zstd_stream::decode_all(payload)
        .map_err(|e| ZbitError::Parse(format!("zstd decode failed: {e}")))?;

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "raw-zstd output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

fn build_raw_xz_payload(input: &[u8], profile: CompressionProfile) -> ZbitResult<Vec<u8>> {
    let mut configs = vec![(0usize, 9u32, 0u32), (1, 9, 3), (2, 9, 4)];
    if profile.enable_xz_extreme_for_raw_xz() {
        let extreme = (1u32 << 31) | 9;
        configs.extend([
            (3usize, extreme, 0u32),
            (4, extreme, 3u32),
            (5, extreme, 4u32),
        ]);
    }

    let results = configs
        .into_par_iter()
        .map(|(rank, preset, profile_pb)| {
            let payload = if profile_pb == 0 {
                xz_encode_easy_preset(input, preset)?
            } else {
                xz_encode_with_profile(input, preset, profile_pb)?
            };
            Ok::<_, ZbitError>((rank, payload))
        })
        .collect::<ZbitResult<Vec<_>>>()?;

    let (_, best) = results
        .into_iter()
        .min_by_key(|(rank, payload)| (payload.len(), *rank))
        .ok_or_else(|| ZbitError::Internal("raw-xz candidate set is empty".to_string()))?;
    Ok(best)
}

fn decode_raw_xz_payload(payload: &[u8], original_size: usize) -> ZbitResult<Vec<u8>> {
    let out = xz_decode_all(payload)?;
    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "raw-xz output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }
    Ok(out)
}

fn model_blob_for_symbol(symbol: u8) -> ZbitResult<Vec<u8>> {
    let mut outputs = [0u8; 8];
    for i in 0..8u32 {
        outputs[i as usize] = if ((symbol >> i) & 1) != 0 { 1 } else { 0 };
    }

    let mut model = ZbitModel::new(3)?;
    model.compress_from_table(&outputs, None)?;
    Ok(model.to_bytes())
}

fn decode_symbol_from_blob(blob: &[u8]) -> ZbitResult<u8> {
    let model = ZbitModel::from_bytes(blob)?;
    let table = model.decompress_to_table()?;
    if table.len() != 8 {
        return Err(ZbitError::Internal(
            "symbol model did not decode to 8-entry table".to_string(),
        ));
    }

    let mut value = 0u8;
    for i in 0..8u8 {
        if table[i as usize] != 0 {
            value |= 1u8 << i;
        }
    }
    Ok(value)
}

fn build_circuit_blobs(symbols: &[u8]) -> ZbitResult<(Vec<Vec<u8>>, usize)> {
    let mut blobs = Vec::with_capacity(symbols.len());
    let mut total_bytes = 0usize;

    for &symbol in symbols {
        let blob = model_blob_for_symbol(symbol)?;
        total_bytes += 1 + 4 + blob.len();
        blobs.push(blob);
    }

    Ok((blobs, total_bytes))
}

fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u8> {
    let b = *bytes
        .get(*cursor)
        .ok_or_else(|| ZbitError::Parse("unexpected end of pack".to_string()))?;
    *cursor += 1;
    Ok(b)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u16> {
    let slice = bytes
        .get(*cursor..(*cursor + 2))
        .ok_or_else(|| ZbitError::Parse("unexpected end of pack".to_string()))?;
    *cursor += 2;
    Ok(u16::from_le_bytes([slice[0], slice[1]]))
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u32> {
    let slice = bytes
        .get(*cursor..(*cursor + 4))
        .ok_or_else(|| ZbitError::Parse("unexpected end of pack".to_string()))?;
    *cursor += 4;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u64> {
    let slice = bytes
        .get(*cursor..(*cursor + 8))
        .ok_or_else(|| ZbitError::Parse("unexpected end of pack".to_string()))?;
    *cursor += 8;
    Ok(u64::from_le_bytes([
        slice[0], slice[1], slice[2], slice[3], slice[4], slice[5], slice[6], slice[7],
    ]))
}

fn push_varint_u64(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
}

fn read_varint_u64(bytes: &[u8], cursor: &mut usize) -> ZbitResult<u64> {
    let mut shift = 0u32;
    let mut value = 0u64;

    for _ in 0..10 {
        let byte = read_u8(bytes, cursor)?;
        let chunk = (byte & 0x7F) as u64;
        let shifted = chunk.checked_shl(shift).ok_or_else(|| {
            ZbitError::Parse("varint shift overflow while decoding u64".to_string())
        })?;
        value |= shifted;
        if (byte & 0x80) == 0 {
            return Ok(value);
        }
        shift = shift.saturating_add(7);
    }

    Err(ZbitError::Parse(
        "varint exceeds 10-byte u64 representation".to_string(),
    ))
}

fn max_value_for_width(width: usize) -> Option<u64> {
    if width == 0 || width > 8 {
        return None;
    }
    if width == 8 {
        return Some(u64::MAX);
    }
    Some((1u64 << (width * 8)) - 1)
}

fn read_le_u64_width(slice: &[u8], width: usize) -> Option<u64> {
    if width == 0 || width > 8 || slice.len() != width {
        return None;
    }
    let mut value = 0u64;
    for (shift, &byte) in slice.iter().enumerate() {
        value |= (byte as u64) << (shift * 8);
    }
    Some(value)
}

fn write_le_u64_width(out: &mut Vec<u8>, value: u64, width: usize) -> ZbitResult<()> {
    let max = max_value_for_width(width).ok_or_else(|| {
        ZbitError::Internal("monotonic-delta width must be between 1 and 8".to_string())
    })?;
    if value > max {
        return Err(ZbitError::Parse(format!(
            "monotonic-delta value {value} does not fit in {width} bytes"
        )));
    }
    for shift in 0..width {
        out.push(((value >> (shift * 8)) & 0xFF) as u8);
    }
    Ok(())
}

fn encode_zigzag_i64(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

fn decode_zigzag_i64(value: u64) -> Option<i64> {
    if value > ((i64::MAX as u64) << 1) + 1 {
        return None;
    }
    Some(((value >> 1) as i64) ^ (-((value & 1) as i64)))
}

fn encode_gap_varints(gaps: &[u64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(gaps.len());
    for &gap in gaps {
        push_varint_u64(&mut out, gap);
    }
    out
}

fn encode_gap_bytes(gaps: &[u64]) -> Option<Vec<u8>> {
    if gaps.iter().any(|&gap| gap > u8::MAX as u64) {
        return None;
    }
    let mut out = Vec::with_capacity(gaps.len());
    for &gap in gaps {
        out.push(gap as u8);
    }
    Some(out)
}

fn encode_gap_delta_varints(gaps: &[u64]) -> Option<Vec<u8>> {
    if gaps.is_empty() {
        return Some(Vec::new());
    }
    if gaps.iter().any(|&gap| gap > i64::MAX as u64) {
        return None;
    }

    let mut out = Vec::with_capacity(gaps.len());
    push_varint_u64(&mut out, gaps[0]);

    let mut prev = gaps[0] as i64;
    for &gap_u64 in gaps.iter().skip(1) {
        let gap = gap_u64 as i64;
        let delta = gap - prev;
        push_varint_u64(&mut out, encode_zigzag_i64(delta));
        prev = gap;
    }

    Some(out)
}

fn common_suffix_trailing_zero_shift(gaps: &[u64]) -> u8 {
    gaps.iter()
        .skip(1)
        .copied()
        .filter(|&gap| gap > 0)
        .map(u64::trailing_zeros)
        .min()
        .unwrap_or(0)
        .min(63) as u8
}

fn encode_trailing_zero_gap_varints(gaps: &[u64], shift: u8) -> Option<Vec<u8>> {
    if gaps.is_empty() || shift == 0 || shift >= 64 {
        return None;
    }

    let mut out = Vec::with_capacity(gaps.len());
    push_varint_u64(&mut out, gaps[0]);
    for &gap in gaps.iter().skip(1) {
        if (gap & ((1u64 << shift) - 1)) != 0 {
            return None;
        }
        push_varint_u64(&mut out, gap >> shift);
    }
    Some(out)
}

fn encode_trailing_zero_gap_bytes(gaps: &[u64], shift: u8) -> Option<Vec<u8>> {
    if gaps.is_empty() || gaps[0] > u8::MAX as u64 || shift == 0 || shift >= 64 {
        return None;
    }

    let mut out = Vec::with_capacity(gaps.len());
    out.push(gaps[0] as u8);
    for &gap in gaps.iter().skip(1) {
        if (gap & ((1u64 << shift) - 1)) != 0 {
            return None;
        }
        let scaled = gap >> shift;
        if scaled == 0 || scaled > u8::MAX as u64 {
            return None;
        }
        out.push(scaled as u8);
    }
    Some(out)
}

const MONOTONIC_DELTA_DICT_BYTES: usize = 28;

fn monotonic_delta_dictionary_size(_stream: &MonotonicDeltaStream) -> usize {
    MONOTONIC_DELTA_DICT_BYTES
}

fn write_monotonic_delta_dictionary(out: &mut Vec<u8>, stream: &MonotonicDeltaStream) {
    out.push(stream.width);
    out.push(stream.mode.as_u8());
    out.push(stream.codec.as_u8());
    out.push(stream.trailing_zero_shift);
    push_u64(out, stream.count);
    push_u64(out, stream.first_value);
    push_u64(out, stream.transformed_plain_len as u64);
}

fn build_monotonic_delta_stream(
    input: &[u8],
    context: &mut CompressionContext,
) -> ZbitResult<Option<MonotonicDeltaStream>> {
    let candidate_widths = [3usize, 4, 2, 5, 6, 7, 8, 1];
    let mut best: Option<(MonotonicDeltaStream, usize)> = None;

    for width in candidate_widths {
        if input.is_empty() || input.len() < width * 3 || (input.len() % width) != 0 {
            continue;
        }

        let count = input.len() / width;
        if count < 2 {
            continue;
        }

        let mut values = Vec::with_capacity(count);
        let mut valid = true;
        for chunk in input.chunks_exact(width) {
            let Some(value) = read_le_u64_width(chunk, width) else {
                valid = false;
                break;
            };
            values.push(value);
        }
        if !valid {
            continue;
        }

        let mut gaps = Vec::with_capacity(count.saturating_sub(1));
        let mut monotonic = true;
        for idx in 1..values.len() {
            if values[idx] <= values[idx - 1] {
                monotonic = false;
                break;
            }
            gaps.push(values[idx] - values[idx - 1]);
        }
        if !monotonic {
            continue;
        }

        let mut mode_candidates = Vec::new();
        if let Some(bytes) = encode_gap_bytes(&gaps) {
            mode_candidates.push((MonotonicDeltaMode::GapBytes, bytes));
        }
        mode_candidates.push((MonotonicDeltaMode::GapVarint, encode_gap_varints(&gaps)));
        if let Some(bytes) = encode_gap_delta_varints(&gaps) {
            mode_candidates.push((MonotonicDeltaMode::GapDeltaVarint, bytes));
        }
        let trailing_zero_shift = common_suffix_trailing_zero_shift(&gaps);
        if let Some(bytes) = encode_trailing_zero_gap_bytes(&gaps, trailing_zero_shift) {
            mode_candidates.push((MonotonicDeltaMode::GapTrailingZeroBytes, bytes));
        }
        if let Some(bytes) = encode_trailing_zero_gap_varints(&gaps, trailing_zero_shift) {
            mode_candidates.push((MonotonicDeltaMode::GapTrailingZeroVarint, bytes));
        }

        for (mode, transformed) in mode_candidates {
            let (codec, payload) =
                choose_best_codec_cached(&transformed, true, true, context)?;
            let shift = match mode {
                MonotonicDeltaMode::GapTrailingZeroBytes
                | MonotonicDeltaMode::GapTrailingZeroVarint => trailing_zero_shift,
                MonotonicDeltaMode::GapBytes
                | MonotonicDeltaMode::GapVarint
                | MonotonicDeltaMode::GapDeltaVarint => 0,
            };
            let stream = MonotonicDeltaStream {
                width: width as u8,
                count: count as u64,
                first_value: values[0],
                transformed_plain_len: transformed.len(),
                mode,
                trailing_zero_shift: shift,
                codec,
                payload,
            };
            let total_size = monotonic_delta_dictionary_size(&stream) + stream.payload.len();
            match &best {
                Some((_, best_size)) if *best_size <= total_size => {}
                _ => best = Some((stream, total_size)),
            }
        }
    }

    Ok(best.map(|(stream, _)| stream))
}

fn decode_monotonic_delta_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    if dict_bytes.len() != MONOTONIC_DELTA_DICT_BYTES {
        return Err(ZbitError::Parse(format!(
            "monotonic-delta dictionary size must be {MONOTONIC_DELTA_DICT_BYTES} bytes"
        )));
    }

    let mut dict_cursor = 0usize;
    let width = read_u8(dict_bytes, &mut dict_cursor)? as usize;
    let mode =
        MonotonicDeltaMode::from_u8(read_u8(dict_bytes, &mut dict_cursor)?).ok_or_else(|| {
            ZbitError::Parse("monotonic-delta dictionary has invalid mode".to_string())
        })?;
    let codec = PayloadCodec::from_u8(read_u8(dict_bytes, &mut dict_cursor)?).ok_or_else(|| {
        ZbitError::Parse("monotonic-delta dictionary has invalid codec".to_string())
    })?;
    let trailing_zero_shift = read_u8(dict_bytes, &mut dict_cursor)?;
    let count = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let first_value = read_u64(dict_bytes, &mut dict_cursor)?;
    let transformed_plain_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;

    if dict_cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in monotonic-delta dictionary".to_string(),
        ));
    }
    if width == 0 || width > 8 {
        return Err(ZbitError::Parse(
            "monotonic-delta width must be between 1 and 8".to_string(),
        ));
    }

    let expected_original = count
        .checked_mul(width)
        .ok_or_else(|| ZbitError::Parse("monotonic-delta output length overflow".to_string()))?;
    if expected_original != original_size {
        return Err(ZbitError::Parse(format!(
            "monotonic-delta output length mismatch: expected {original_size} got {expected_original}"
        )));
    }
    if count == 0 {
        if original_size == 0 {
            return Ok(Vec::new());
        }
        return Err(ZbitError::Parse(
            "monotonic-delta count is zero for non-empty payload".to_string(),
        ));
    }

    let transformed = decode_with_codec(payload, codec, transformed_plain_len)?;
    let mut transformed_cursor = 0usize;
    let max_value = max_value_for_width(width).ok_or_else(|| {
        ZbitError::Parse("monotonic-delta width does not have a valid value range".to_string())
    })?;

    let mut out = Vec::with_capacity(original_size);
    write_le_u64_width(&mut out, first_value, width)?;
    let mut current = first_value;

    let mut prev_gap = 0u64;
    for idx in 1..count {
        let gap = match mode {
            MonotonicDeltaMode::GapBytes => read_u8(&transformed, &mut transformed_cursor)? as u64,
            MonotonicDeltaMode::GapVarint => {
                read_varint_u64(&transformed, &mut transformed_cursor)?
            }
            MonotonicDeltaMode::GapTrailingZeroBytes => {
                if idx == 1 {
                    read_u8(&transformed, &mut transformed_cursor)? as u64
                } else {
                    let scaled = read_u8(&transformed, &mut transformed_cursor)? as u64;
                    scaled
                        .checked_shl(trailing_zero_shift as u32)
                        .ok_or_else(|| {
                            ZbitError::Parse(
                                "monotonic-delta trailing-zero byte gap overflow".to_string(),
                            )
                        })?
                }
            }
            MonotonicDeltaMode::GapTrailingZeroVarint => {
                if idx == 1 {
                    read_varint_u64(&transformed, &mut transformed_cursor)?
                } else {
                    let scaled = read_varint_u64(&transformed, &mut transformed_cursor)?;
                    scaled
                        .checked_shl(trailing_zero_shift as u32)
                        .ok_or_else(|| {
                            ZbitError::Parse(
                                "monotonic-delta trailing-zero varint gap overflow".to_string(),
                            )
                        })?
                }
            }
            MonotonicDeltaMode::GapDeltaVarint => {
                if idx == 1 {
                    let first_gap = read_varint_u64(&transformed, &mut transformed_cursor)?;
                    prev_gap = first_gap;
                    first_gap
                } else {
                    let encoded_delta = read_varint_u64(&transformed, &mut transformed_cursor)?;
                    let delta = decode_zigzag_i64(encoded_delta).ok_or_else(|| {
                        ZbitError::Parse(
                            "monotonic-delta gap-delta zigzag value exceeds i64 range".to_string(),
                        )
                    })? as i128;
                    let next_gap = (prev_gap as i128).checked_add(delta).ok_or_else(|| {
                        ZbitError::Parse(
                            "monotonic-delta gap-delta overflow while decoding".to_string(),
                        )
                    })?;
                    if next_gap <= 0 || next_gap > u64::MAX as i128 {
                        return Err(ZbitError::Parse(
                            "monotonic-delta decoded non-positive gap".to_string(),
                        ));
                    }
                    let gap = next_gap as u64;
                    prev_gap = gap;
                    gap
                }
            }
        };

        if gap == 0 {
            return Err(ZbitError::Parse(
                "monotonic-delta gap must be strictly positive".to_string(),
            ));
        }

        current = current.checked_add(gap).ok_or_else(|| {
            ZbitError::Parse("monotonic-delta value overflow while decoding".to_string())
        })?;
        if current > max_value {
            return Err(ZbitError::Parse(
                "monotonic-delta decoded value exceeds width capacity".to_string(),
            ));
        }
        write_le_u64_width(&mut out, current, width)?;
    }

    if transformed_cursor != transformed.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in monotonic-delta transformed payload".to_string(),
        ));
    }

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "monotonic-delta output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

fn push_u32_be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn read_u32_be_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn parse_crc32_frame_at(input: &[u8], start: usize) -> Option<(u32, [u8; 4], usize, usize)> {
    let frame_len_u32 = read_u32_be_at(input, start)?;
    let frame_len = frame_len_u32 as usize;
    let tag_off = start.checked_add(4)?;
    let tag_slice = input.get(tag_off..tag_off + 4)?;
    let tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
    let data_off = tag_off + 4;
    let data_end = data_off.checked_add(frame_len)?;
    let crc_off = data_end;
    let next = crc_off.checked_add(4)?;
    if next > input.len() {
        return None;
    }
    let data = input.get(data_off..data_end)?;
    let crc = read_u32_be_at(input, crc_off)?;
    let mut hasher = Crc32Hasher::new();
    hasher.update(&tag);
    hasher.update(data);
    if hasher.finalize() != crc {
        return None;
    }
    Some((frame_len_u32, tag, data_off, next))
}

fn build_framed_payload_run(input: &[u8]) -> Option<FramedPayloadRun> {
    if input.len() < 24 {
        return None;
    }

    let mut best: Option<(usize, FramedPayloadRun)> = None;
    let mut start = 0usize;

    while start + 12 <= input.len() {
        let Some((first_len_u32, first_tag, first_data_off, first_next)) =
            parse_crc32_frame_at(input, start)
        else {
            start += 1;
            continue;
        };

        let mut chunk_lengths = vec![first_len_u32];
        let mut payload = Vec::<u8>::new();
        payload
            .extend_from_slice(input.get(first_data_off..first_data_off + first_len_u32 as usize)?);

        let mut cursor = first_next;
        while let Some((len_u32, tag, data_off, next)) = parse_crc32_frame_at(input, cursor) {
            if tag != first_tag {
                break;
            }
            chunk_lengths.push(len_u32);
            payload.extend_from_slice(input.get(data_off..data_off + len_u32 as usize)?);
            cursor = next;
        }

        if chunk_lengths.len() < 2 {
            start += 1;
            continue;
        }

        let total_chunks = u32::try_from(chunk_lengths.len()).ok()?;
        let base_chunk_len = chunk_lengths[0];
        let mut full_chunk_count = total_chunks;
        let mut tail_chunk_len = 0u32;

        if chunk_lengths.iter().any(|&len| len != base_chunk_len) {
            if chunk_lengths
                .iter()
                .take(chunk_lengths.len().saturating_sub(1))
                .any(|&len| len != base_chunk_len)
            {
                start += 1;
                continue;
            }
            full_chunk_count = total_chunks.saturating_sub(1);
            tail_chunk_len = *chunk_lengths.last().unwrap_or(&0u32);
        }

        let run = FramedPayloadRun {
            prefix: input[..start].to_vec(),
            suffix: input[cursor..].to_vec(),
            frame_tag: first_tag,
            payload,
            base_chunk_len,
            full_chunk_count,
            tail_chunk_len,
            total_chunks,
        };

        let candidate_size = ZBPK_HEADER_BYTES + framed_dictionary_size(&run) + run.payload.len();
        match &best {
            Some((best_size, _)) if *best_size <= candidate_size => {}
            _ => best = Some((candidate_size, run)),
        }

        start += 1;
    }

    best.map(|(_, run)| run)
}

fn framed_dictionary_size(stream: &FramedPayloadRun) -> usize {
    28usize + stream.prefix.len() + stream.suffix.len()
}

fn write_framed_dictionary(out: &mut Vec<u8>, stream: &FramedPayloadRun) {
    push_u32(out, stream.prefix.len() as u32);
    push_u32(out, stream.suffix.len() as u32);
    out.extend_from_slice(&stream.frame_tag);
    push_u32(out, stream.base_chunk_len);
    push_u32(out, stream.full_chunk_count);
    push_u32(out, stream.tail_chunk_len);
    push_u32(out, stream.total_chunks);
    out.extend_from_slice(&stream.prefix);
    out.extend_from_slice(&stream.suffix);
}

fn decode_framed_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut dict_cursor = 0usize;
    let prefix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let suffix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tag_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 4)
        .ok_or_else(|| ZbitError::Parse("framed-raw missing frame tag".to_string()))?;
    dict_cursor += 4;
    let frame_tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
    let base_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let full_chunk_count = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tail_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let total_chunks = read_u32(dict_bytes, &mut dict_cursor)? as usize;

    let prefix = dict_bytes
        .get(dict_cursor..dict_cursor + prefix_len)
        .ok_or_else(|| ZbitError::Parse("framed-raw prefix range out of bounds".to_string()))?;
    dict_cursor += prefix_len;

    let suffix = dict_bytes
        .get(dict_cursor..dict_cursor + suffix_len)
        .ok_or_else(|| ZbitError::Parse("framed-raw suffix range out of bounds".to_string()))?;
    dict_cursor += suffix_len;

    if dict_cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in framed-raw dictionary".to_string(),
        ));
    }

    let tail_present = if total_chunks == full_chunk_count {
        false
    } else if total_chunks == full_chunk_count + 1 {
        true
    } else {
        return Err(ZbitError::Parse(
            "framed-raw dictionary has inconsistent chunk counters".to_string(),
        ));
    };

    let expected_payload = full_chunk_count
        .checked_mul(base_chunk_len)
        .and_then(|v| {
            if tail_present {
                v.checked_add(tail_chunk_len)
            } else {
                Some(v)
            }
        })
        .ok_or_else(|| ZbitError::Parse("framed-raw payload length overflow".to_string()))?;

    if payload.len() != expected_payload {
        return Err(ZbitError::Parse(format!(
            "framed-raw payload length mismatch: expected {expected_payload} got {}",
            payload.len()
        )));
    }

    let chunk_overhead = total_chunks
        .checked_mul(12)
        .ok_or_else(|| ZbitError::Parse("framed-raw chunk overhead overflow".to_string()))?;
    let mut out = Vec::with_capacity(
        prefix
            .len()
            .checked_add(payload.len())
            .and_then(|v| v.checked_add(suffix.len()))
            .and_then(|v| v.checked_add(chunk_overhead))
            .ok_or_else(|| ZbitError::Parse("framed-raw output length overflow".to_string()))?,
    );

    out.extend_from_slice(prefix);

    let mut payload_cursor = 0usize;
    for idx in 0..total_chunks {
        let chunk_len = if idx < full_chunk_count {
            base_chunk_len
        } else {
            tail_chunk_len
        };
        let chunk_data = payload
            .get(payload_cursor..payload_cursor + chunk_len)
            .ok_or_else(|| {
                ZbitError::Parse("framed-raw payload chunk range out of bounds".to_string())
            })?;
        payload_cursor += chunk_len;

        push_u32_be(&mut out, chunk_len as u32);
        out.extend_from_slice(&frame_tag);
        out.extend_from_slice(chunk_data);

        let mut hasher = Crc32Hasher::new();
        hasher.update(&frame_tag);
        hasher.update(chunk_data);
        push_u32_be(&mut out, hasher.finalize());
    }

    out.extend_from_slice(suffix);

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "framed-raw output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

fn apply_unary_delta(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; data.len()];
    out[0] = data[0];
    for i in 1..data.len() {
        out[i] = data[i].wrapping_sub(data[i - 1]);
    }
    out
}

fn invert_unary_delta(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; data.len()];
    out[0] = data[0];
    for i in 1..data.len() {
        out[i] = data[i].wrapping_add(out[i - 1]);
    }
    out
}

fn apply_unary_xor(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; data.len()];
    out[0] = data[0];
    for i in 1..data.len() {
        out[i] = data[i] ^ data[i - 1];
    }
    out
}

fn invert_unary_xor(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let mut out = vec![0u8; data.len()];
    out[0] = data[0];
    for i in 1..data.len() {
        out[i] = data[i] ^ out[i - 1];
    }
    out
}

fn apply_periodic_head_tail(data: &[u8], period: usize, head: usize) -> Option<Vec<u8>> {
    if period < 2 || head == 0 || head >= period {
        return None;
    }
    let mut heads = Vec::with_capacity(data.len() / period * head + period);
    let mut tails = Vec::with_capacity(data.len().saturating_sub(heads.capacity()));
    let mut cursor = 0usize;
    while cursor < data.len() {
        let end = (cursor + period).min(data.len());
        let chunk = &data[cursor..end];
        let h = head.min(chunk.len());
        heads.extend_from_slice(&chunk[..h]);
        tails.extend_from_slice(&chunk[h..]);
        cursor = end;
    }
    heads.extend_from_slice(&tails);
    Some(heads)
}

fn invert_periodic_head_tail(
    data: &[u8],
    original_len: usize,
    period: usize,
    head: usize,
) -> Option<Vec<u8>> {
    if period < 2 || head == 0 || head >= period || data.len() != original_len {
        return None;
    }
    let mut total_head = 0usize;
    let mut cursor = 0usize;
    while cursor < original_len {
        let end = (cursor + period).min(original_len);
        total_head = total_head.checked_add(head.min(end - cursor))?;
        cursor = end;
    }

    let head_stream = data.get(..total_head)?;
    let tail_stream = data.get(total_head..)?;
    let mut out = Vec::with_capacity(original_len);

    let mut h_cur = 0usize;
    let mut t_cur = 0usize;
    let mut pos = 0usize;
    while pos < original_len {
        let end = (pos + period).min(original_len);
        let chunk_len = end - pos;
        let h = head.min(chunk_len);
        let t = chunk_len - h;

        out.extend_from_slice(head_stream.get(h_cur..h_cur + h)?);
        out.extend_from_slice(tail_stream.get(t_cur..t_cur + t)?);
        h_cur += h;
        t_cur += t;
        pos = end;
    }

    if out.len() == original_len {
        Some(out)
    } else {
        None
    }
}

fn apply_periodic_gather(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 2 || period > data.len() {
        return None;
    }
    let mut out = Vec::with_capacity(data.len());
    for offset in 0..period {
        let mut cursor = offset;
        while cursor < data.len() {
            out.push(data[cursor]);
            cursor = cursor.saturating_add(period);
        }
    }
    Some(out)
}

fn invert_periodic_gather(data: &[u8], original_len: usize, period: usize) -> Option<Vec<u8>> {
    if period < 2 || period > original_len || data.len() != original_len {
        return None;
    }
    let mut out = vec![0u8; original_len];
    let mut read_cursor = 0usize;
    for offset in 0..period {
        let mut cursor = offset;
        while cursor < original_len {
            out[cursor] = *data.get(read_cursor)?;
            read_cursor += 1;
            cursor = cursor.saturating_add(period);
        }
    }
    if read_cursor != data.len() {
        return None;
    }
    Some(out)
}

fn apply_periodic_delta(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 2 || period > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    for i in period..data.len() {
        out[i] = data[i].wrapping_sub(data[i - period]);
    }
    Some(out)
}

fn invert_periodic_delta(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 2 || period > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    for i in period..data.len() {
        out[i] = data[i].wrapping_add(out[i - period]);
    }
    Some(out)
}

fn apply_periodic_xor(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 2 || period > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    for i in period..data.len() {
        out[i] = data[i] ^ data[i - period];
    }
    Some(out)
}

fn invert_periodic_xor(data: &[u8], period: usize) -> Option<Vec<u8>> {
    if period < 2 || period > data.len() {
        return None;
    }
    let mut out = data.to_vec();
    for i in period..data.len() {
        out[i] = data[i] ^ out[i - period];
    }
    Some(out)
}

fn periodic_head_bytes(original_len: usize, period: usize, head: usize) -> Option<usize> {
    if period < 2 || head == 0 || head >= period {
        return None;
    }
    let mut total_head = 0usize;
    let mut cursor = 0usize;
    while cursor < original_len {
        let end = (cursor + period).min(original_len);
        total_head = total_head.checked_add(head.min(end - cursor))?;
        cursor = end;
    }
    Some(total_head)
}

fn apply_head_tail_tail_gather(
    data: &[u8],
    period: usize,
    tail_gather_period: usize,
) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, 1)?;
    let total_head = periodic_head_bytes(data.len(), period, 1)?;
    let head_stream = head_tail.get(..total_head)?;
    let tail_stream = head_tail.get(total_head..)?;
    let tail_gathered = apply_periodic_gather(tail_stream, tail_gather_period)?;
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(head_stream);
    out.extend_from_slice(&tail_gathered);
    Some(out)
}

fn invert_head_tail_tail_gather(
    data: &[u8],
    original_len: usize,
    period: usize,
    tail_gather_period: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let total_head = periodic_head_bytes(original_len, period, 1)?;
    let head_stream = data.get(..total_head)?;
    let tail_stream = data.get(total_head..)?;
    let tail = invert_periodic_gather(tail_stream, original_len - total_head, tail_gather_period)?;
    let mut merged = Vec::with_capacity(original_len);
    merged.extend_from_slice(head_stream);
    merged.extend_from_slice(&tail);
    invert_periodic_head_tail(&merged, original_len, period, 1)
}

fn apply_head_tail_tail_gather_delta(
    data: &[u8],
    period: usize,
    tail_gather_period: usize,
) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, 1)?;
    let total_head = periodic_head_bytes(data.len(), period, 1)?;
    let head_stream = head_tail.get(..total_head)?;
    let tail_stream = head_tail.get(total_head..)?;
    let tail_gathered = apply_periodic_gather(tail_stream, tail_gather_period)?;
    let tail_delta = apply_unary_delta(&tail_gathered);
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(head_stream);
    out.extend_from_slice(&tail_delta);
    Some(out)
}

fn invert_head_tail_tail_gather_delta(
    data: &[u8],
    original_len: usize,
    period: usize,
    tail_gather_period: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let total_head = periodic_head_bytes(original_len, period, 1)?;
    let head_stream = data.get(..total_head)?;
    let tail_delta = data.get(total_head..)?;
    let tail_gathered = invert_unary_delta(tail_delta);
    let tail = invert_periodic_gather(
        &tail_gathered,
        original_len - total_head,
        tail_gather_period,
    )?;
    let mut merged = Vec::with_capacity(original_len);
    merged.extend_from_slice(head_stream);
    merged.extend_from_slice(&tail);
    invert_periodic_head_tail(&merged, original_len, period, 1)
}

fn apply_head_tail_tail_delta(
    data: &[u8],
    period: usize,
    tail_delta_period: usize,
) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, 1)?;
    let total_head = periodic_head_bytes(data.len(), period, 1)?;
    let head_stream = head_tail.get(..total_head)?;
    let tail_stream = head_tail.get(total_head..)?;
    let tail_delta = apply_periodic_delta(tail_stream, tail_delta_period)?;
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(head_stream);
    out.extend_from_slice(&tail_delta);
    Some(out)
}

fn invert_head_tail_tail_delta(
    data: &[u8],
    original_len: usize,
    period: usize,
    tail_delta_period: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let total_head = periodic_head_bytes(original_len, period, 1)?;
    let head_stream = data.get(..total_head)?;
    let tail_delta = data.get(total_head..)?;
    let tail = invert_periodic_delta(tail_delta, tail_delta_period)?;
    let mut merged = Vec::with_capacity(original_len);
    merged.extend_from_slice(head_stream);
    merged.extend_from_slice(&tail);
    invert_periodic_head_tail(&merged, original_len, period, 1)
}

fn apply_head_tail_tail_xor(data: &[u8], period: usize, tail_xor_period: usize) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, 1)?;
    let total_head = periodic_head_bytes(data.len(), period, 1)?;
    let head_stream = head_tail.get(..total_head)?;
    let tail_stream = head_tail.get(total_head..)?;
    let tail_xor = apply_periodic_xor(tail_stream, tail_xor_period)?;
    let mut out = Vec::with_capacity(data.len());
    out.extend_from_slice(head_stream);
    out.extend_from_slice(&tail_xor);
    Some(out)
}

fn invert_head_tail_tail_xor(
    data: &[u8],
    original_len: usize,
    period: usize,
    tail_xor_period: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let total_head = periodic_head_bytes(original_len, period, 1)?;
    let head_stream = data.get(..total_head)?;
    let tail_xor = data.get(total_head..)?;
    let tail = invert_periodic_xor(tail_xor, tail_xor_period)?;
    let mut merged = Vec::with_capacity(original_len);
    merged.extend_from_slice(head_stream);
    merged.extend_from_slice(&tail);
    invert_periodic_head_tail(&merged, original_len, period, 1)
}

fn apply_head_tail_delta(data: &[u8], period: usize, head: usize) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, head)?;
    Some(apply_unary_delta(&head_tail))
}

fn invert_head_tail_delta(
    data: &[u8],
    original_len: usize,
    period: usize,
    head: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let recovered = invert_unary_delta(data);
    invert_periodic_head_tail(&recovered, original_len, period, head)
}

fn apply_head_tail_xor(data: &[u8], period: usize, head: usize) -> Option<Vec<u8>> {
    let head_tail = apply_periodic_head_tail(data, period, head)?;
    Some(apply_unary_xor(&head_tail))
}

fn invert_head_tail_xor(
    data: &[u8],
    original_len: usize,
    period: usize,
    head: usize,
) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    let recovered = invert_unary_xor(data);
    invert_periodic_head_tail(&recovered, original_len, period, head)
}

fn apply_bit_plane_transpose(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    let n = data.len();
    let mut out = vec![0u8; n];
    for (byte_idx, &byte) in data.iter().enumerate() {
        for bit in 0..8usize {
            if ((byte >> bit) & 1) == 0 {
                continue;
            }
            let dst_bit_index = bit * n + byte_idx;
            let dst_byte = dst_bit_index >> 3;
            let dst_bit = dst_bit_index & 7;
            out[dst_byte] |= 1u8 << dst_bit;
        }
    }
    out
}

fn invert_bit_plane_transpose(data: &[u8], original_len: usize) -> Option<Vec<u8>> {
    if data.len() != original_len {
        return None;
    }
    if data.is_empty() {
        return Some(Vec::new());
    }
    let n = original_len;
    let mut out = vec![0u8; n];
    for bit in 0..8usize {
        for byte_idx in 0..n {
            let src_bit_index = bit.checked_mul(n).and_then(|v| v.checked_add(byte_idx))?;
            let src_byte = *data.get(src_bit_index >> 3)?;
            let src_bit = (src_byte >> (src_bit_index & 7)) & 1;
            if src_bit != 0 {
                out[byte_idx] |= 1u8 << bit;
            }
        }
    }
    Some(out)
}

fn apply_transform_plan(data: &[u8], plan: &CircuitTransformPlan) -> Option<Vec<u8>> {
    match plan.kind {
        CircuitTransformKind::Identity => Some(data.to_vec()),
        CircuitTransformKind::DeltaPrev => Some(apply_unary_delta(data)),
        CircuitTransformKind::XorPrev => Some(apply_unary_xor(data)),
        CircuitTransformKind::BitPlaneTranspose => Some(apply_bit_plane_transpose(data)),
        CircuitTransformKind::BitPlaneTransposeDelta => {
            let transposed = apply_bit_plane_transpose(data);
            Some(apply_unary_delta(&transposed))
        }
        CircuitTransformKind::BitPlaneTransposeXor => {
            let transposed = apply_bit_plane_transpose(data);
            Some(apply_unary_xor(&transposed))
        }
        CircuitTransformKind::PeriodicHeadTail => {
            apply_periodic_head_tail(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicGather => apply_periodic_gather(data, plan.period as usize),
        CircuitTransformKind::PeriodicDelta => apply_periodic_delta(data, plan.period as usize),
        CircuitTransformKind::PeriodicXor => apply_periodic_xor(data, plan.period as usize),
        CircuitTransformKind::PeriodicGatherDelta => {
            let gathered = apply_periodic_gather(data, plan.period as usize)?;
            Some(apply_unary_delta(&gathered))
        }
        CircuitTransformKind::PeriodicGatherXor => {
            let gathered = apply_periodic_gather(data, plan.period as usize)?;
            Some(apply_unary_xor(&gathered))
        }
        CircuitTransformKind::PeriodicHeadTailTailGather => {
            apply_head_tail_tail_gather(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailTailGatherDelta => {
            apply_head_tail_tail_gather_delta(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailTailDelta => {
            apply_head_tail_tail_delta(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailTailXor => {
            apply_head_tail_tail_xor(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailDelta => {
            apply_head_tail_delta(data, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailXor => {
            apply_head_tail_xor(data, plan.period as usize, plan.head as usize)
        }
    }
}

fn invert_transform_plan(
    data: &[u8],
    original_len: usize,
    plan: &CircuitTransformPlan,
) -> Option<Vec<u8>> {
    match plan.kind {
        CircuitTransformKind::Identity => {
            if data.len() == original_len {
                Some(data.to_vec())
            } else {
                None
            }
        }
        CircuitTransformKind::DeltaPrev => {
            if data.len() == original_len {
                Some(invert_unary_delta(data))
            } else {
                None
            }
        }
        CircuitTransformKind::XorPrev => {
            if data.len() == original_len {
                Some(invert_unary_xor(data))
            } else {
                None
            }
        }
        CircuitTransformKind::BitPlaneTranspose => invert_bit_plane_transpose(data, original_len),
        CircuitTransformKind::BitPlaneTransposeDelta => {
            if data.len() != original_len {
                return None;
            }
            let recovered = invert_unary_delta(data);
            invert_bit_plane_transpose(&recovered, original_len)
        }
        CircuitTransformKind::BitPlaneTransposeXor => {
            if data.len() != original_len {
                return None;
            }
            let recovered = invert_unary_xor(data);
            invert_bit_plane_transpose(&recovered, original_len)
        }
        CircuitTransformKind::PeriodicHeadTail => {
            invert_periodic_head_tail(data, original_len, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicGather => {
            invert_periodic_gather(data, original_len, plan.period as usize)
        }
        CircuitTransformKind::PeriodicDelta => {
            if data.len() == original_len {
                invert_periodic_delta(data, plan.period as usize)
            } else {
                None
            }
        }
        CircuitTransformKind::PeriodicXor => {
            if data.len() == original_len {
                invert_periodic_xor(data, plan.period as usize)
            } else {
                None
            }
        }
        CircuitTransformKind::PeriodicGatherDelta => {
            if data.len() != original_len {
                return None;
            }
            let recovered = invert_unary_delta(data);
            invert_periodic_gather(&recovered, original_len, plan.period as usize)
        }
        CircuitTransformKind::PeriodicGatherXor => {
            if data.len() != original_len {
                return None;
            }
            let recovered = invert_unary_xor(data);
            invert_periodic_gather(&recovered, original_len, plan.period as usize)
        }
        CircuitTransformKind::PeriodicHeadTailTailGather => invert_head_tail_tail_gather(
            data,
            original_len,
            plan.period as usize,
            plan.head as usize,
        ),
        CircuitTransformKind::PeriodicHeadTailTailGatherDelta => {
            invert_head_tail_tail_gather_delta(
                data,
                original_len,
                plan.period as usize,
                plan.head as usize,
            )
        }
        CircuitTransformKind::PeriodicHeadTailTailDelta => invert_head_tail_tail_delta(
            data,
            original_len,
            plan.period as usize,
            plan.head as usize,
        ),
        CircuitTransformKind::PeriodicHeadTailTailXor => {
            invert_head_tail_tail_xor(data, original_len, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailDelta => {
            invert_head_tail_delta(data, original_len, plan.period as usize, plan.head as usize)
        }
        CircuitTransformKind::PeriodicHeadTailXor => {
            invert_head_tail_xor(data, original_len, plan.period as usize, plan.head as usize)
        }
    }
}

fn xz_encode_easy_preset(data: &[u8], preset: u32) -> ZbitResult<Vec<u8>> {
    let stream = Stream::new_easy_encoder(preset, Check::None)
        .map_err(|e| ZbitError::Io(format!("xz easy stream init failed: {e}")))?;
    let mut encoder = XzWriterEncoder::new_stream(Vec::new(), stream);
    encoder
        .write_all(data)
        .map_err(|e| ZbitError::Io(format!("xz easy encode write failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| ZbitError::Io(format!("xz easy encode finish failed: {e}")))
}

fn xz_encode_with_profile(
    data: &[u8],
    preset: u32,
    literal_context_bits: u32,
) -> ZbitResult<Vec<u8>> {
    let mut options = LzmaOptions::new_preset(preset)
        .map_err(|e| ZbitError::Io(format!("xz options preset init failed: {e}")))?;
    options.dict_size(64 * 1024 * 1024);
    options.literal_context_bits(literal_context_bits);
    options.literal_position_bits(0);
    options.position_bits(2);
    options.mode(Mode::Normal);
    options.nice_len(273);
    options.match_finder(MatchFinder::BinaryTree4);
    options.depth(0);

    let mut filters = Filters::new();
    filters.lzma2(&options);

    let stream = Stream::new_stream_encoder(&filters, Check::None)
        .map_err(|e| ZbitError::Io(format!("xz stream init failed: {e}")))?;

    let mut encoder = XzWriterEncoder::new_stream(Vec::new(), stream);
    encoder
        .write_all(data)
        .map_err(|e| ZbitError::Io(format!("xz encode write failed: {e}")))?;
    encoder
        .finish()
        .map_err(|e| ZbitError::Io(format!("xz encode finish failed: {e}")))
}

fn xz_decode_all(data: &[u8]) -> ZbitResult<Vec<u8>> {
    let mut decoder = XzDecoder::new(Cursor::new(data));
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| ZbitError::Parse(format!("xz decode failed: {e}")))?;
    Ok(out)
}

fn zstd_encode_with_level(data: &[u8], level: i32) -> ZbitResult<Vec<u8>> {
    zstd_stream::encode_all(data, level)
        .map_err(|e| ZbitError::Io(format!("zstd encode failed: {e}")))
}

fn zstd_decode_exact(data: &[u8], expected_len: usize) -> ZbitResult<Vec<u8>> {
    let out = zstd_stream::decode_all(data)
        .map_err(|e| ZbitError::Parse(format!("zstd decode failed: {e}")))?;
    if out.len() != expected_len {
        return Err(ZbitError::Parse(format!(
            "zstd output length mismatch: expected {expected_len} got {}",
            out.len()
        )));
    }
    Ok(out)
}

fn decode_with_codec(data: &[u8], codec: PayloadCodec, expected_len: usize) -> ZbitResult<Vec<u8>> {
    match codec {
        PayloadCodec::Raw => {
            if data.len() != expected_len {
                return Err(ZbitError::Parse(format!(
                    "raw codec length mismatch: expected {expected_len} got {}",
                    data.len()
                )));
            }
            Ok(data.to_vec())
        }
        PayloadCodec::Xz | PayloadCodec::XzExtreme => {
            let out = xz_decode_all(data)?;
            if out.len() != expected_len {
                return Err(ZbitError::Parse(format!(
                    "xz output length mismatch: expected {expected_len} got {}",
                    out.len()
                )));
            }
            Ok(out)
        }
        PayloadCodec::Zstd => zstd_decode_exact(data, expected_len),
    }
}

fn choose_best_codec(
    data: &[u8],
    allow_raw: bool,
    allow_xz_extreme: bool,
    profile: CompressionProfile,
) -> ZbitResult<(PayloadCodec, Vec<u8>)> {
    let mut codec_jobs = vec![
        (0usize, PayloadCodec::Xz, 9u32, 0u32),
        (1usize, PayloadCodec::Xz, 9u32, 3u32),
        (2usize, PayloadCodec::Xz, 9u32, 4u32),
    ];
    if allow_xz_extreme && profile.enable_xz_extreme_refinement() {
        let extreme = (1u32 << 31) | 9;
        codec_jobs.extend([
            (3usize, PayloadCodec::XzExtreme, extreme, 0u32),
            (4usize, PayloadCodec::XzExtreme, extreme, 3u32),
            (5usize, PayloadCodec::XzExtreme, extreme, 4u32),
        ]);
    }

    let mut candidates = codec_jobs
        .into_par_iter()
        .map(|(rank, codec, preset, profile_pb)| {
            let payload = if profile_pb == 0 {
                xz_encode_easy_preset(data, preset)?
            } else {
                xz_encode_with_profile(data, preset, profile_pb)?
            };
            Ok::<_, ZbitError>((rank, codec, payload))
        })
        .collect::<ZbitResult<Vec<_>>>()?;

    let zstd_level = match profile {
        CompressionProfile::Fast => 10,
        CompressionProfile::Balanced => 17,
        CompressionProfile::Deep | CompressionProfile::Research => 19,
    };
    let zstd = zstd_encode_with_level(data, zstd_level)?;
    candidates.push((10usize, PayloadCodec::Zstd, zstd));

    if allow_raw {
        candidates.push((11usize, PayloadCodec::Raw, data.to_vec()));
    }

    let (_, codec, payload) = candidates
        .into_iter()
        .min_by_key(|(rank, _, payload)| (payload.len(), *rank))
        .ok_or_else(|| ZbitError::Internal("codec candidate set is empty".to_string()))?;
    Ok((codec, payload))
}

fn choose_best_codec_cached(
    data: &[u8],
    allow_raw: bool,
    allow_xz_extreme: bool,
    context: &mut CompressionContext,
) -> ZbitResult<(PayloadCodec, Vec<u8>)> {
    let key = (
        payload_hash(data),
        CodecProfileKey {
            allow_raw,
            allow_xz_extreme,
            profile: context.profile,
        },
    );
    if let Some((codec, payload)) = context.cache.codec_outputs.get(&key) {
        context.cache_stats.codec_hits = context.cache_stats.codec_hits.saturating_add(1);
        return Ok((*codec, payload.clone()));
    }

    context.cache_stats.codec_misses = context.cache_stats.codec_misses.saturating_add(1);
    let (codec, payload) = choose_best_codec(data, allow_raw, allow_xz_extreme, context.profile)?;
    context
        .cache
        .codec_outputs
        .insert(key, (codec, payload.clone()));
    Ok((codec, payload))
}

fn score_periodic_candidates(data: &[u8], max_period: usize, top_k: usize) -> Vec<usize> {
    if data.len() < 2048 || max_period < 2 || top_k == 0 {
        return Vec::new();
    }

    let n = data.len();
    let max_p = max_period.min(n.saturating_sub(1));
    let target_samples = 20_000usize.min(n.saturating_sub(2));
    if target_samples == 0 {
        return Vec::new();
    }

    let mut scored = Vec::<(usize, f64)>::new();
    for period in 2..=max_p {
        let range = n - period;
        let step = (range / target_samples).max(1);
        let mut matches = 0usize;
        let mut total = 0usize;
        let mut idx = period;
        while idx < n {
            if data[idx] == data[idx - period] {
                matches += 1;
            }
            total += 1;
            idx = idx.saturating_add(step);
        }
        if total == 0 {
            continue;
        }
        let score = matches as f64 / total as f64;
        scored.push((period, score));
    }

    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    scored.truncate(top_k);
    scored.into_iter().map(|(p, _)| p).collect()
}

const TOPOLOGY_HASH_OFFSET: u64 = 0xcbf29ce484222325;
const TOPOLOGY_HASH_PRIME: u64 = 0x100000001b3;
const TOPOLOGY_NODE_BYTES: usize = 28;
const TOPOLOGY_CORRECTION_PLAN_KIND_BASE: u8 = 200;

fn topology_hash_mix(mut state: u64, value: u64) -> u64 {
    state ^= value;
    state.wrapping_mul(TOPOLOGY_HASH_PRIME)
}

fn finalize_topology_hashes(nodes: &mut [CircuitTopologyNode]) -> ZbitResult<()> {
    let mut hash_by_id = HashMap::<u32, u64>::new();
    for node in nodes.iter_mut() {
        let parent_hash = if node.parent_id == u32::MAX {
            TOPOLOGY_HASH_OFFSET
        } else {
            *hash_by_id.get(&node.parent_id).ok_or_else(|| {
                ZbitError::Internal("topology contains child before parent".to_string())
            })?
        };
        let mut hash = TOPOLOGY_HASH_OFFSET;
        hash = topology_hash_mix(hash, parent_hash);
        hash = topology_hash_mix(hash, node.id as u64);
        hash = topology_hash_mix(hash, node.parent_id as u64);
        hash = topology_hash_mix(hash, node.relation as u64);
        hash = topology_hash_mix(hash, node.order as u64);
        hash = topology_hash_mix(hash, node.kind as u64);
        hash = topology_hash_mix(hash, node.param_a as u64);
        hash = topology_hash_mix(hash, node.param_b as u64);
        node.hash64 = hash;
        hash_by_id.insert(node.id, hash);
    }
    Ok(())
}

fn push_topology_node(
    nodes: &mut Vec<CircuitTopologyNode>,
    parent_id: u32,
    relation: u8,
    order: u16,
    kind: u8,
    param_a: u32,
    param_b: u32,
) -> u32 {
    let id = (nodes.len() as u32).saturating_add(1);
    nodes.push(CircuitTopologyNode {
        id,
        parent_id,
        relation,
        order,
        kind,
        param_a,
        param_b,
        hash64: 0,
    });
    id
}

fn embed_correction_plan_in_topology(
    nodes: &mut Vec<CircuitTopologyNode>,
    correction_plan: &CircuitTransformPlan,
) -> ZbitResult<bool> {
    if correction_plan.kind == CircuitTransformKind::Identity
        && correction_plan.period == 0
        && correction_plan.head == 0
    {
        return Ok(false);
    }

    let root_id = nodes
        .iter()
        .find(|node| node.parent_id == u32::MAX)
        .map(|node| node.id)
        .ok_or_else(|| ZbitError::Internal("topology root node missing".to_string()))?;
    let order = nodes
        .iter()
        .filter(|node| node.parent_id == root_id && node.relation == 0)
        .map(|node| node.order)
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let kind = TOPOLOGY_CORRECTION_PLAN_KIND_BASE.saturating_add(correction_plan.kind.as_u8());
    let _ = push_topology_node(
        nodes,
        root_id,
        0,
        order,
        kind,
        correction_plan.period,
        correction_plan.head,
    );
    finalize_topology_hashes(nodes)?;
    Ok(true)
}

fn decode_embedded_correction_plan(
    kind: u8,
    period: u32,
    head: u32,
) -> Option<CircuitTransformPlan> {
    let kind_u8 = kind.checked_sub(TOPOLOGY_CORRECTION_PLAN_KIND_BASE)?;
    let transform_kind = CircuitTransformKind::from_u8(kind_u8)?;
    Some(CircuitTransformPlan {
        kind: transform_kind,
        period,
        head,
    })
}

fn build_topology_for_plan(plan: &CircuitTransformPlan) -> ZbitResult<Vec<CircuitTopologyNode>> {
    let mut nodes = Vec::new();
    let root_id = push_topology_node(&mut nodes, u32::MAX, 0, 0, 0, 0, 0);

    match plan.kind {
        CircuitTransformKind::Identity => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 1, 0, 0);
        }
        CircuitTransformKind::DeltaPrev => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 2, 1, 0);
        }
        CircuitTransformKind::XorPrev => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 3, 1, 0);
        }
        CircuitTransformKind::BitPlaneTranspose => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 19, 8, 0);
        }
        CircuitTransformKind::BitPlaneTransposeDelta => {
            let bitplane_id = push_topology_node(&mut nodes, root_id, 0, 0, 19, 8, 0);
            let _ = push_topology_node(&mut nodes, bitplane_id, 0, 0, 20, 1, 0);
        }
        CircuitTransformKind::BitPlaneTransposeXor => {
            let bitplane_id = push_topology_node(&mut nodes, root_id, 0, 0, 19, 8, 0);
            let _ = push_topology_node(&mut nodes, bitplane_id, 0, 0, 21, 1, 0);
        }
        CircuitTransformKind::PeriodicHeadTail => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, plan.head);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, plan.head, 0);
            let _ = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(plan.head),
                0,
            );
        }
        CircuitTransformKind::PeriodicGather => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 5, plan.period, 0);
        }
        CircuitTransformKind::PeriodicDelta => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 6, plan.period, 0);
        }
        CircuitTransformKind::PeriodicXor => {
            let _ = push_topology_node(&mut nodes, root_id, 0, 0, 7, plan.period, 0);
        }
        CircuitTransformKind::PeriodicGatherDelta => {
            let gather_id = push_topology_node(&mut nodes, root_id, 0, 0, 5, plan.period, 0);
            let _ = push_topology_node(&mut nodes, gather_id, 0, 0, 12, 1, 0);
        }
        CircuitTransformKind::PeriodicGatherXor => {
            let gather_id = push_topology_node(&mut nodes, root_id, 0, 0, 5, plan.period, 0);
            let _ = push_topology_node(&mut nodes, gather_id, 0, 0, 13, 1, 0);
        }
        CircuitTransformKind::PeriodicHeadTailTailGather => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, 1);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, 1, 0);
            let tail_id = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(1),
                0,
            );
            let _ = push_topology_node(&mut nodes, tail_id, 0, 0, 14, plan.head, 0);
        }
        CircuitTransformKind::PeriodicHeadTailTailGatherDelta => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, 1);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, 1, 0);
            let tail_id = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(1),
                0,
            );
            let gather_id = push_topology_node(&mut nodes, tail_id, 0, 0, 14, plan.head, 0);
            let _ = push_topology_node(&mut nodes, gather_id, 0, 0, 12, 1, 0);
        }
        CircuitTransformKind::PeriodicHeadTailTailDelta => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, 1);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, 1, 0);
            let tail_id = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(1),
                0,
            );
            let _ = push_topology_node(&mut nodes, tail_id, 0, 0, 15, plan.head, 0);
        }
        CircuitTransformKind::PeriodicHeadTailTailXor => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, 1);
            let _ = push_topology_node(&mut nodes, split_id, 1, 0, 10, 1, 0);
            let tail_id = push_topology_node(
                &mut nodes,
                split_id,
                1,
                1,
                11,
                plan.period.saturating_sub(1),
                0,
            );
            let _ = push_topology_node(&mut nodes, tail_id, 0, 0, 16, plan.head, 0);
        }
        CircuitTransformKind::PeriodicHeadTailDelta => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, plan.head);
            let _ = push_topology_node(&mut nodes, split_id, 0, 0, 17, 1, 0);
        }
        CircuitTransformKind::PeriodicHeadTailXor => {
            let split_id = push_topology_node(&mut nodes, root_id, 0, 0, 4, plan.period, plan.head);
            let _ = push_topology_node(&mut nodes, split_id, 0, 0, 18, 1, 0);
        }
    }

    finalize_topology_hashes(&mut nodes)?;
    Ok(nodes)
}

#[derive(Debug, Clone)]
struct AdaptiveTransformResult {
    plan: CircuitTransformPlan,
    topology: Vec<CircuitTopologyNode>,
    codec: PayloadCodec,
    payload: Vec<u8>,
    sampling_ms: f64,
    eval_ms: f64,
}

fn choose_adaptive_transform_plan(
    data: &[u8],
    profile: CompressionProfile,
    selected_budget: usize,
) -> ZbitResult<AdaptiveTransformResult> {
    let mut plans = vec![
        CircuitTransformPlan {
            kind: CircuitTransformKind::Identity,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::DeltaPrev,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::XorPrev,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTranspose,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTransposeDelta,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTransposeXor,
            period: 0,
            head: 0,
        },
    ];

    let mut add_plan = |plan: CircuitTransformPlan| {
        if !plans.iter().any(|p| *p == plan) {
            plans.push(plan);
        }
    };

    let high_corr_periods = score_periodic_candidates(data, 8192, 8);
    let mut period_candidates = high_corr_periods.clone();
    period_candidates.extend([2usize, 3, 4, 5, 8, 16, 32, 64, 128, 257, 512, 1024, 2048]);
    period_candidates.sort_unstable();
    period_candidates.dedup();

    for period in period_candidates {
        if period < 2 || period > data.len() {
            continue;
        }
        let period_u32 = period as u32;
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicGather,
            period: period_u32,
            head: 0,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicDelta,
            period: period_u32,
            head: 0,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicXor,
            period: period_u32,
            head: 0,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicGatherDelta,
            period: period_u32,
            head: 0,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicGatherXor,
            period: period_u32,
            head: 0,
        });

        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTail,
            period: period_u32,
            head: 1,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTailDelta,
            period: period_u32,
            head: 1,
        });
        add_plan(CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTailXor,
            period: period_u32,
            head: 1,
        });
        for tail_gather_period in [2u32, 4, 8] {
            if (period_u32.saturating_sub(1)) >= tail_gather_period && tail_gather_period >= 2 {
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailTailGather,
                    period: period_u32,
                    head: tail_gather_period,
                });
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailTailGatherDelta,
                    period: period_u32,
                    head: tail_gather_period,
                });
            }
        }
        let mut tail_delta_periods = vec![2u32, 4, 8, 16];
        let full_tail_period = period_u32.saturating_sub(1);
        if full_tail_period >= 2 {
            tail_delta_periods.push(full_tail_period);
        }
        let half_tail_period = full_tail_period / 2;
        if half_tail_period >= 2 {
            tail_delta_periods.push(half_tail_period);
        }
        tail_delta_periods.sort_unstable();
        tail_delta_periods.dedup();

        for tail_delta_period in tail_delta_periods {
            if (period_u32.saturating_sub(1)) >= tail_delta_period && tail_delta_period >= 2 {
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailTailDelta,
                    period: period_u32,
                    head: tail_delta_period,
                });
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailTailXor,
                    period: period_u32,
                    head: tail_delta_period,
                });
            }
        }
        if period > 4 {
            add_plan(CircuitTransformPlan {
                kind: CircuitTransformKind::PeriodicHeadTail,
                period: period_u32,
                head: 2,
            });
            add_plan(CircuitTransformPlan {
                kind: CircuitTransformKind::PeriodicHeadTailDelta,
                period: period_u32,
                head: 2,
            });
            add_plan(CircuitTransformPlan {
                kind: CircuitTransformKind::PeriodicHeadTailXor,
                period: period_u32,
                head: 2,
            });
        }
        if period > 8 {
            let half = (period / 2) as u32;
            if half > 0 && half < period_u32 {
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTail,
                    period: period_u32,
                    head: half,
                });
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailDelta,
                    period: period_u32,
                    head: half,
                });
                add_plan(CircuitTransformPlan {
                    kind: CircuitTransformKind::PeriodicHeadTailXor,
                    period: period_u32,
                    head: half,
                });
            }
        }
    }

    let sample_len = data.len().min(512 * 1024);
    let sample = &data[..sample_len];

    let sample_timer = Instant::now();
    let quick_zstd_level = match profile {
        CompressionProfile::Fast | CompressionProfile::Balanced => 1,
        CompressionProfile::Deep => 3,
        CompressionProfile::Research => 5,
    };
    let scored = plans
        .par_iter()
        .map(|plan| {
            let Some(transformed_sample) = apply_transform_plan(sample, plan) else {
                return Ok::<_, ZbitError>(None);
            };
            let quick = zstd_encode_with_level(&transformed_sample, quick_zstd_level)?.len();
            Ok(Some((*plan, quick)))
        })
        .collect::<ZbitResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let sampling_ms = sample_timer.elapsed().as_secs_f64() * 1000.0;

    let mut scored = scored;
    scored.sort_unstable_by_key(|entry| entry.1);
    let mut selected = Vec::<CircuitTransformPlan>::new();
    let mut selected_set = HashSet::<CircuitTransformPlan>::new();
    let bounded_budget = selected_budget
        .max(1)
        .min(profile.max_transform_plans().max(1));
    for (plan, _) in scored.iter().take(bounded_budget) {
        if selected_set.insert(*plan) {
            selected.push(*plan);
        }
    }

    for (idx, period) in high_corr_periods.into_iter().take(2).enumerate() {
        let plan = CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTail,
            period: period as u32,
            head: 1,
        };
        if selected_set.insert(plan) {
            selected.push(plan);
        }
        if idx == 0 {
            let period_u32 = period as u32;
            let mut forced_tail_heads = vec![4u32];
            let full_tail = period_u32.saturating_sub(1);
            if full_tail >= 2 {
                forced_tail_heads.push(full_tail);
            }
            let half_tail = full_tail / 2;
            if half_tail >= 2 {
                forced_tail_heads.push(half_tail);
            }
            forced_tail_heads.sort_unstable();
            forced_tail_heads.dedup();

            for forced_kind in [
                CircuitTransformKind::PeriodicHeadTailDelta,
                CircuitTransformKind::PeriodicHeadTailXor,
            ] {
                let forced = CircuitTransformPlan {
                    kind: forced_kind,
                    period: period_u32,
                    head: 4,
                };
                if selected_set.insert(forced) {
                    selected.push(forced);
                }
            }

            for head in forced_tail_heads {
                for forced_kind in [
                    CircuitTransformKind::PeriodicHeadTailTailDelta,
                    CircuitTransformKind::PeriodicHeadTailTailXor,
                    CircuitTransformKind::PeriodicHeadTailTailGather,
                ] {
                    let forced = CircuitTransformPlan {
                        kind: forced_kind,
                        period: period_u32,
                        head,
                    };
                    if selected_set.insert(forced) {
                        selected.push(forced);
                    }
                }
            }
        }
    }

    for baseline in [
        CircuitTransformPlan {
            kind: CircuitTransformKind::Identity,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::DeltaPrev,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::XorPrev,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTranspose,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTransposeDelta,
            period: 0,
            head: 0,
        },
        CircuitTransformPlan {
            kind: CircuitTransformKind::BitPlaneTransposeXor,
            period: 0,
            head: 0,
        },
    ] {
        if selected_set.insert(baseline) {
            selected.push(baseline);
        }
    }

    let trace_recursive = std::env::var_os("ZBIT_TRACE_RECURSIVE").is_some();
    let eval_timer = Instant::now();
    let eval_results = selected
        .iter()
        .copied()
        .enumerate()
        .collect::<Vec<_>>()
        .into_par_iter()
        .map(|(rank, plan)| {
            let Some(transformed) = apply_transform_plan(data, &plan) else {
                return Ok::<_, ZbitError>(None);
            };
            let (codec, final_payload) = choose_best_codec(&transformed, true, false, profile)?;
            if trace_recursive {
                eprintln!(
                    "zbit-trace plan-candidate kind={} period={} head={} encoded={} codec={}",
                    plan.kind.name(),
                    plan.period,
                    plan.head,
                    final_payload.len(),
                    codec.name()
                );
            }
            Ok(Some((rank, plan, transformed, codec, final_payload)))
        })
        .collect::<ZbitResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let eval_ms = eval_timer.elapsed().as_secs_f64() * 1000.0;

    let (_, plan, transformed, mut codec, mut encoded) = eval_results
        .into_iter()
        .min_by_key(|(rank, _, _, _, final_payload)| (final_payload.len(), *rank))
        .ok_or_else(|| {
            ZbitError::Internal("failed to evaluate adaptive transform plans".to_string())
        })?;
    let topology = build_topology_for_plan(&plan)?;

    if profile.enable_xz_extreme_refinement() {
        let extreme = (1u32 << 31) | 9;
        let xz_extreme_candidates = [
            (0usize, extreme, 0u32),
            (1usize, extreme, 3u32),
            (2usize, extreme, 4u32),
        ]
        .into_par_iter()
        .map(|(rank, preset, pb)| {
            let encoded = if pb == 0 {
                xz_encode_easy_preset(&transformed, preset)?
            } else {
                xz_encode_with_profile(&transformed, preset, pb)?
            };
            Ok::<_, ZbitError>((rank, encoded))
        })
        .collect::<ZbitResult<Vec<_>>>()?;
        if let Some((_, better)) = xz_extreme_candidates
            .into_iter()
            .min_by_key(|(rank, bytes)| (bytes.len(), *rank))
        {
            if better.len() < encoded.len() {
                codec = PayloadCodec::XzExtreme;
                encoded = better;
            }
        }
    }

    Ok(AdaptiveTransformResult {
        plan,
        topology,
        codec,
        payload: encoded,
        sampling_ms,
        eval_ms,
    })
}

fn choose_correction_transform_plan(
    corrections: &[u8],
    profile: CompressionProfile,
) -> ZbitResult<(CircuitTransformPlan, PayloadCodec, Vec<u8>, f64, f64)> {
    let identity_plan = CircuitTransformPlan {
        kind: CircuitTransformKind::Identity,
        period: 0,
        head: 0,
    };
    let (identity_codec, identity_payload) = choose_best_codec(corrections, true, true, profile)?;
    let candidate = choose_adaptive_transform_plan(
        corrections,
        profile,
        profile.correction_transform_plan_budget(),
    )?;
    let candidate_plan = candidate.plan;
    let candidate_codec = candidate.codec;
    let candidate_payload = candidate.payload;

    let candidate_total = candidate_payload.len()
        + if candidate_plan == identity_plan {
            0
        } else {
            TOPOLOGY_NODE_BYTES
        };

    if candidate_total < identity_payload.len() {
        Ok((
            candidate_plan,
            candidate_codec,
            candidate_payload,
            candidate.sampling_ms,
            candidate.eval_ms,
        ))
    } else {
        Ok((identity_plan, identity_codec, identity_payload, 0.0, 0.0))
    }
}

fn preflate_chain_candidates(profile: CompressionProfile) -> Vec<u32> {
    let default = match profile {
        CompressionProfile::Fast => vec![4096u32],
        CompressionProfile::Balanced => vec![4096u32, 8192, 16384],
        CompressionProfile::Deep => vec![4096u32, 8192, 16384, 24576],
        CompressionProfile::Research => vec![2048u32, 4096, 8192, 16384, 24576, 32768],
    };
    let Some(raw) = std::env::var_os("ZBIT_PREFLATE_CHAIN_CANDIDATES") else {
        return default;
    };

    let mut out = Vec::new();
    for token in raw.to_string_lossy().split(',') {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(value) = trimmed.parse::<u32>() {
            if value >= 256 {
                out.push(value);
            }
        }
    }

    out.sort_unstable();
    out.dedup();
    if out.is_empty() {
        default
    } else {
        out
    }
}

fn build_recursive_circuit_stream(
    _input: &[u8],
    base: &FramedPayloadRun,
    context: &mut CompressionContext,
) -> ZbitResult<Option<RecursiveCircuitStream>> {
    if base.payload.len() < 6 {
        context.push_skipped("recursive-circuit-xz skipped: framed payload smaller than 6 bytes");
        return Ok(None);
    }
    let recursive_total_timer = Instant::now();

    let mut zlib_header = [0u8; 2];
    zlib_header.copy_from_slice(&base.payload[..2]);
    let mut zlib_adler32 = [0u8; 4];
    zlib_adler32.copy_from_slice(&base.payload[base.payload.len() - 4..]);
    let deflate_stream = &base.payload[2..base.payload.len() - 4];
    let deflate_stream_hash = payload_hash(deflate_stream);

    let preflate_timer = Instant::now();
    let mut preflate_results = Vec::new();
    let mut missing_chain_lengths = Vec::new();
    for max_chain_length in preflate_chain_candidates(context.profile) {
        let cache_key = (deflate_stream_hash, max_chain_length);
        if let Some(cached) = context.cache.preflate_outputs.get(&cache_key) {
            context.cache_stats.preflate_hits = context.cache_stats.preflate_hits.saturating_add(1);
            if let Some((corrections, plain)) = cached {
                preflate_results.push((max_chain_length, corrections.clone(), plain.clone()));
            }
            continue;
        }

        context.cache_stats.preflate_misses = context.cache_stats.preflate_misses.saturating_add(1);
        missing_chain_lengths.push(max_chain_length);
    }

    let evaluated_missing = missing_chain_lengths
        .into_par_iter()
        .map(|max_chain_length| {
            let mut config = PreflateConfig::default();
            config.verify_compression = true;
            config.plain_text_limit = ZBPK_MAX_OUTPUT_BYTES;
            config.max_chain_length = max_chain_length;
            let evaluated = match preflate_whole_deflate_stream(deflate_stream, &config) {
                Ok((chunk, plain)) => Some((chunk.corrections, plain.text().to_vec())),
                Err(_) => None,
            };
            (max_chain_length, evaluated)
        })
        .collect::<Vec<_>>();
    for (max_chain_length, evaluated) in evaluated_missing {
        let cache_key = (deflate_stream_hash, max_chain_length);
        context
            .cache
            .preflate_outputs
            .insert(cache_key, evaluated.clone());
        if let Some((corrections, plain)) = evaluated {
            preflate_results.push((max_chain_length, corrections, plain));
        }
    }
    preflate_results.sort_unstable_by_key(|(max_chain_length, _, _)| *max_chain_length);
    context.timings.recursive_preflate_ms += preflate_timer.elapsed().as_secs_f64() * 1000.0;
    if preflate_results.is_empty() {
        context.push_skipped(
            "recursive-circuit-xz unavailable: preflate reconstruction failed for all chain candidates"
        );
        context.timings.recursive_total_ms += recursive_total_timer.elapsed().as_secs_f64() * 1000.0;
        return Ok(None);
    }

    let plain_len = preflate_results[0].2.len();
    let transformed_template = choose_adaptive_transform_plan(
        &preflate_results[0].2,
        context.profile,
        context.profile.max_transform_plans(),
    )?;
    context.timings.recursive_transform_sampling_ms += transformed_template.sampling_ms;
    context.timings.recursive_transform_eval_ms += transformed_template.eval_ms;

    let correction_timer = Instant::now();
    let trace_recursive = std::env::var_os("ZBIT_TRACE_RECURSIVE").is_some();
    let profile = context.profile;
    let evaluated = preflate_results
        .into_par_iter()
        .map(|(max_chain_length, corrections, _)| {
            let (
                correction_plan,
                correction_codec,
                corrections_payload,
                _correction_sample_ms,
                _correction_eval_ms,
            ) = choose_correction_transform_plan(&corrections, profile)?;

            let mut topology = transformed_template.topology.clone();
            let _ = embed_correction_plan_in_topology(&mut topology, &correction_plan)?;

            let stream = RecursiveCircuitStream {
                base: base.clone(),
                transformed_payload: transformed_template.payload.clone(),
                corrections_payload,
                plain_len,
                transformed_encoded_len: transformed_template.payload.len(),
                correction_plain_len: corrections.len(),
                correction_encoded_len: 0,
                transformed_codec: transformed_template.codec,
                correction_codec,
                zlib_header,
                zlib_adler32,
                transform_plan: transformed_template.plan,
                topology,
            };
            let correction_encoded_len = stream.corrections_payload.len();
            let mut stream = stream;
            stream.correction_encoded_len = correction_encoded_len;

            let candidate_total = ZBPK_HEADER_BYTES
                + recursive_circuit_dictionary_size(&stream)
                + stream.transformed_encoded_len
                + stream.correction_encoded_len;
            if trace_recursive {
                eprintln!(
                    "zbit-trace recursive chain={} plan={} period={} head={} transformed={}({}) corrections={}({}) corr-plain={} total={}",
                    max_chain_length,
                    stream.transform_plan.kind.name(),
                    stream.transform_plan.period,
                    stream.transform_plan.head,
                    stream.transformed_encoded_len,
                    stream.transformed_codec.name(),
                    stream.correction_encoded_len,
                    stream.correction_codec.name(),
                    stream.correction_plain_len,
                    candidate_total,
                );
                eprintln!(
                    "zbit-trace recursive correction-plan={} period={} head={}",
                    correction_plan.kind.name(),
                    correction_plan.period,
                    correction_plan.head
                );
            }
            Ok::<_, ZbitError>((max_chain_length, stream, candidate_total))
        })
        .collect::<ZbitResult<Vec<_>>>()?;
    context.timings.recursive_correction_modeling_ms +=
        correction_timer.elapsed().as_secs_f64() * 1000.0;

    let Some((_max_chain_length, stream, _)) = evaluated
        .into_iter()
        .min_by_key(|(max_chain_length, _, candidate_total)| (*candidate_total, *max_chain_length))
    else {
        context.push_skipped("recursive-circuit-xz unavailable: no valid correction model candidate");
        context.timings.recursive_total_ms += recursive_total_timer.elapsed().as_secs_f64() * 1000.0;
        return Ok(None);
    };

    let trace_recursive = std::env::var_os("ZBIT_TRACE_RECURSIVE").is_some();

    if trace_recursive {
        eprintln!(
            "zbit-trace recursive selected chain plan={} period={} head={} transformed={}({}) corrections={}({}) plain={}",
            stream.transform_plan.kind.name(),
            stream.transform_plan.period,
            stream.transform_plan.head,
            stream.transformed_encoded_len,
            stream.transformed_codec.name(),
            stream.correction_encoded_len,
            stream.correction_codec.name(),
            plain_len,
        );
    }
    context.timings.recursive_total_ms += recursive_total_timer.elapsed().as_secs_f64() * 1000.0;
    Ok(Some(stream))
}

fn recursive_circuit_dictionary_size(stream: &RecursiveCircuitStream) -> usize {
    framed_dictionary_size(&stream.base) + 51 + stream.topology.len() * TOPOLOGY_NODE_BYTES
}

fn write_recursive_circuit_dictionary(out: &mut Vec<u8>, stream: &RecursiveCircuitStream) {
    write_framed_dictionary(out, &stream.base);
    out.extend_from_slice(&stream.zlib_header);
    out.extend_from_slice(&stream.zlib_adler32);
    push_u64(out, stream.plain_len as u64);
    push_u64(out, stream.transformed_encoded_len as u64);
    push_u64(out, stream.correction_plain_len as u64);
    push_u64(out, stream.correction_encoded_len as u64);
    out.push(stream.transformed_codec.as_u8());
    out.push(stream.correction_codec.as_u8());
    out.push(stream.transform_plan.kind.as_u8());
    push_u32(out, stream.transform_plan.period);
    push_u32(out, stream.transform_plan.head);
    push_u16(out, stream.topology.len() as u16);
    for node in &stream.topology {
        push_u32(out, node.id);
        push_u32(out, node.parent_id);
        out.push(node.relation);
        push_u16(out, node.order);
        out.push(node.kind);
        push_u32(out, node.param_a);
        push_u32(out, node.param_b);
        push_u64(out, node.hash64);
    }
}

fn decode_recursive_circuit_payload(
    dict_bytes: &[u8],
    payload: &[u8],
    original_size: usize,
) -> ZbitResult<Vec<u8>> {
    let mut dict_cursor = 0usize;
    let prefix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let suffix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tag_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 4)
        .ok_or_else(|| ZbitError::Parse("recursive-circuit-xz missing frame tag".to_string()))?;
    dict_cursor += 4;
    let frame_tag = [tag_slice[0], tag_slice[1], tag_slice[2], tag_slice[3]];
    let base_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let full_chunk_count = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tail_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let total_chunks = read_u32(dict_bytes, &mut dict_cursor)? as usize;

    let prefix = dict_bytes
        .get(dict_cursor..dict_cursor + prefix_len)
        .ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz prefix range out of bounds".to_string())
        })?;
    dict_cursor += prefix_len;

    let suffix = dict_bytes
        .get(dict_cursor..dict_cursor + suffix_len)
        .ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz suffix range out of bounds".to_string())
        })?;
    dict_cursor += suffix_len;

    let zlib_header_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 2)
        .ok_or_else(|| ZbitError::Parse("recursive-circuit-xz missing zlib header".to_string()))?;
    dict_cursor += 2;
    let mut zlib_header = [0u8; 2];
    zlib_header.copy_from_slice(zlib_header_slice);

    let zlib_adler_slice = dict_bytes
        .get(dict_cursor..dict_cursor + 4)
        .ok_or_else(|| ZbitError::Parse("recursive-circuit-xz missing zlib adler32".to_string()))?;
    dict_cursor += 4;
    let mut zlib_adler32 = [0u8; 4];
    zlib_adler32.copy_from_slice(zlib_adler_slice);

    let plain_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let transformed_encoded_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let correction_plain_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let correction_encoded_len = read_u64(dict_bytes, &mut dict_cursor)? as usize;
    let transformed_codec = PayloadCodec::from_u8(read_u8(dict_bytes, &mut dict_cursor)?)
        .ok_or_else(|| {
            ZbitError::Parse(
                "recursive-circuit-xz dictionary has invalid transformed codec".to_string(),
            )
        })?;
    let correction_codec = PayloadCodec::from_u8(read_u8(dict_bytes, &mut dict_cursor)?)
        .ok_or_else(|| {
            ZbitError::Parse(
                "recursive-circuit-xz dictionary has invalid correction codec".to_string(),
            )
        })?;
    let transform_kind_u8 = read_u8(dict_bytes, &mut dict_cursor)?;
    let transform_kind = CircuitTransformKind::from_u8(transform_kind_u8).ok_or_else(|| {
        ZbitError::Parse("recursive-circuit-xz dictionary has invalid transform kind".to_string())
    })?;
    let transform_period = read_u32(dict_bytes, &mut dict_cursor)?;
    let transform_head = read_u32(dict_bytes, &mut dict_cursor)?;
    let topology_count = read_u16(dict_bytes, &mut dict_cursor)? as usize;
    let mut correction_plan = CircuitTransformPlan {
        kind: CircuitTransformKind::Identity,
        period: 0,
        head: 0,
    };

    let mut seen_root = false;
    let mut last_id = 0u32;
    let mut hash_by_id = HashMap::<u32, u64>::new();
    for idx in 0..topology_count {
        let id = read_u32(dict_bytes, &mut dict_cursor)?;
        let parent_id = read_u32(dict_bytes, &mut dict_cursor)?;
        let relation = read_u8(dict_bytes, &mut dict_cursor)?;
        let order = read_u16(dict_bytes, &mut dict_cursor)?;
        let kind = read_u8(dict_bytes, &mut dict_cursor)?;
        let param_a = read_u32(dict_bytes, &mut dict_cursor)?;
        let param_b = read_u32(dict_bytes, &mut dict_cursor)?;
        let stored_hash = read_u64(dict_bytes, &mut dict_cursor)?;

        if relation > 1 {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz topology relation must be 0 or 1".to_string(),
            ));
        }
        if idx > 0 && id <= last_id {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz topology node ids must be strictly increasing".to_string(),
            ));
        }
        if parent_id == u32::MAX {
            seen_root = true;
        }
        let parent_hash = if parent_id == u32::MAX {
            TOPOLOGY_HASH_OFFSET
        } else {
            *hash_by_id.get(&parent_id).ok_or_else(|| {
                ZbitError::Parse(
                    "recursive-circuit-xz topology references unknown parent".to_string(),
                )
            })?
        };
        let mut expected_hash = TOPOLOGY_HASH_OFFSET;
        expected_hash = topology_hash_mix(expected_hash, parent_hash);
        expected_hash = topology_hash_mix(expected_hash, id as u64);
        expected_hash = topology_hash_mix(expected_hash, parent_id as u64);
        expected_hash = topology_hash_mix(expected_hash, relation as u64);
        expected_hash = topology_hash_mix(expected_hash, order as u64);
        expected_hash = topology_hash_mix(expected_hash, kind as u64);
        expected_hash = topology_hash_mix(expected_hash, param_a as u64);
        expected_hash = topology_hash_mix(expected_hash, param_b as u64);
        if stored_hash != expected_hash {
            return Err(ZbitError::Parse(
                "recursive-circuit-xz topology hash mismatch".to_string(),
            ));
        }
        if let Some(plan) = decode_embedded_correction_plan(kind, param_a, param_b) {
            correction_plan = plan;
        }
        hash_by_id.insert(id, expected_hash);
        last_id = id;
    }

    if dict_cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in recursive-circuit-xz dictionary".to_string(),
        ));
    }
    if topology_count == 0 || !seen_root {
        return Err(ZbitError::Parse(
            "recursive-circuit-xz topology metadata missing valid root".to_string(),
        ));
    }

    let expected_payload = transformed_encoded_len
        .checked_add(correction_encoded_len)
        .ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz payload length overflow".to_string())
        })?;
    if payload.len() != expected_payload {
        return Err(ZbitError::Parse(format!(
            "recursive-circuit-xz payload length mismatch: expected {expected_payload} got {}",
            payload.len()
        )));
    }

    let transformed_payload = &payload[..transformed_encoded_len];
    let corrections_payload = &payload[transformed_encoded_len..];
    let transformed = decode_with_codec(transformed_payload, transformed_codec, plain_len)?;
    let correction_transformed =
        decode_with_codec(corrections_payload, correction_codec, correction_plain_len)?;
    let corrections = invert_transform_plan(
        &correction_transformed,
        correction_plain_len,
        &correction_plan,
    )
    .ok_or_else(|| {
        ZbitError::Parse("recursive-circuit-xz correction stream is invalid".to_string())
    })?;
    let plan = CircuitTransformPlan {
        kind: transform_kind,
        period: transform_period,
        head: transform_head,
    };
    let filtered_plain =
        invert_transform_plan(&transformed, plain_len, &plan).ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz transformed stream is invalid".to_string())
        })?;

    let deflate_stream = recreate_whole_deflate_stream(&filtered_plain, &corrections)
        .map_err(|e| ZbitError::Parse(format!("preflate recreate failed: {e}")))?;

    let mut framed_payload = Vec::with_capacity(
        2usize
            .checked_add(deflate_stream.len())
            .and_then(|v| v.checked_add(4))
            .ok_or_else(|| {
                ZbitError::Parse("recursive-circuit-xz framed payload overflow".to_string())
            })?,
    );
    framed_payload.extend_from_slice(&zlib_header);
    framed_payload.extend_from_slice(&deflate_stream);
    framed_payload.extend_from_slice(&zlib_adler32);

    let tail_present = if total_chunks == full_chunk_count {
        false
    } else if total_chunks == full_chunk_count + 1 {
        true
    } else {
        return Err(ZbitError::Parse(
            "recursive-circuit-xz dictionary has inconsistent chunk counters".to_string(),
        ));
    };

    let expected_framed_payload = full_chunk_count
        .checked_mul(base_chunk_len)
        .and_then(|v| {
            if tail_present {
                v.checked_add(tail_chunk_len)
            } else {
                Some(v)
            }
        })
        .ok_or_else(|| {
            ZbitError::Parse("recursive-circuit-xz framed length overflow".to_string())
        })?;
    if framed_payload.len() != expected_framed_payload {
        return Err(ZbitError::Parse(format!(
            "recursive-circuit-xz framed length mismatch: expected {expected_framed_payload} got {}",
            framed_payload.len()
        )));
    }

    let chunk_overhead = total_chunks.checked_mul(12).ok_or_else(|| {
        ZbitError::Parse("recursive-circuit-xz chunk overhead overflow".to_string())
    })?;
    let mut out = Vec::with_capacity(
        prefix
            .len()
            .checked_add(framed_payload.len())
            .and_then(|v| v.checked_add(suffix.len()))
            .and_then(|v| v.checked_add(chunk_overhead))
            .ok_or_else(|| {
                ZbitError::Parse("recursive-circuit-xz output length overflow".to_string())
            })?,
    );
    out.extend_from_slice(prefix);

    let mut payload_cursor = 0usize;
    for idx in 0..total_chunks {
        let chunk_len = if idx < full_chunk_count {
            base_chunk_len
        } else {
            tail_chunk_len
        };
        let chunk_data = framed_payload
            .get(payload_cursor..payload_cursor + chunk_len)
            .ok_or_else(|| {
                ZbitError::Parse("recursive-circuit-xz frame range out of bounds".to_string())
            })?;
        payload_cursor += chunk_len;

        push_u32_be(&mut out, chunk_len as u32);
        out.extend_from_slice(&frame_tag);
        out.extend_from_slice(chunk_data);

        let mut hasher = Crc32Hasher::new();
        hasher.update(&frame_tag);
        hasher.update(chunk_data);
        push_u32_be(&mut out, hasher.finalize());
    }

    out.extend_from_slice(suffix);

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "recursive-circuit-xz output length mismatch: expected {original_size} got {}",
            out.len()
        )));
    }

    Ok(out)
}

fn write_pack_bytes(
    method: PackMethod,
    input: &[u8],
    stream: &IndexStream,
    circuit_blobs: Option<&[Vec<u8>]>,
    circuit_dict_bytes: usize,
    huffman_stream: Option<&HuffmanStream>,
    raw_deflate_payload: Option<&[u8]>,
    raw_zstd_payload: Option<&[u8]>,
    raw_xz_payload: Option<&[u8]>,
    framed_run: Option<&FramedPayloadRun>,
    recursive_stream: Option<&RecursiveCircuitStream>,
    monotonic_stream: Option<&MonotonicDeltaStream>,
) -> ZbitResult<Vec<u8>> {
    if stream.unique_symbols.len() > u16::MAX as usize {
        return Err(ZbitError::Limit(
            "unique symbol count exceeds pack format limit".to_string(),
        ));
    }

    let (bits_per_symbol, unique_count, dict_size, payload_size) = match method {
        PackMethod::RawCopy => (0u8, 0usize, 0usize, input.len()),
        PackMethod::IndexedRaw => (
            stream.bits_per_symbol,
            stream.unique_symbols.len(),
            stream.unique_symbols.len(),
            stream.payload.len(),
        ),
        PackMethod::IndexedCircuit => (
            stream.bits_per_symbol,
            stream.unique_symbols.len(),
            circuit_dict_bytes,
            stream.payload.len(),
        ),
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.ok_or_else(|| {
                ZbitError::Internal("huffman stream missing for indexed-huffman method".to_string())
            })?;
            (
                0u8,
                hs.symbols.len(),
                hs.symbols.len() * 2,
                hs.payload.len(),
            )
        }
        PackMethod::RawDeflate => {
            let payload = raw_deflate_payload.ok_or_else(|| {
                ZbitError::Internal(
                    "raw-deflate payload missing for raw-deflate method".to_string(),
                )
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::RawZstd => {
            let payload = raw_zstd_payload.ok_or_else(|| {
                ZbitError::Internal("raw-zstd payload missing for raw-zstd method".to_string())
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::RawXz => {
            let payload = raw_xz_payload.ok_or_else(|| {
                ZbitError::Internal("raw-xz payload missing for raw-xz method".to_string())
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::FramedRaw => {
            let run = framed_run.ok_or_else(|| {
                ZbitError::Internal("framed-run missing for framed-raw method".to_string())
            })?;
            (0u8, 0usize, framed_dictionary_size(run), run.payload.len())
        }
        PackMethod::RecursiveCircuitXz => {
            let recursive = recursive_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "recursive stream missing for recursive-circuit-xz method".to_string(),
                )
            })?;
            (
                0u8,
                0usize,
                recursive_circuit_dictionary_size(recursive),
                recursive.transformed_encoded_len + recursive.correction_encoded_len,
            )
        }
        PackMethod::MonotonicDelta => {
            let monotonic = monotonic_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "monotonic stream missing for monotonic-delta method".to_string(),
                )
            })?;
            (
                0u8,
                0usize,
                monotonic_delta_dictionary_size(monotonic),
                monotonic.payload.len(),
            )
        }
    };

    let mut out = Vec::with_capacity(ZBPK_HEADER_BYTES + dict_size + payload_size);

    push_u32(&mut out, ZBPK_MAGIC);
    push_u16(&mut out, ZBPK_VERSION);
    push_u16(&mut out, 0);
    out.push(method.as_u8());
    out.push(bits_per_symbol);
    push_u16(&mut out, unique_count as u16);
    push_u64(&mut out, input.len() as u64);
    push_u64(&mut out, dict_size as u64);
    push_u64(&mut out, payload_size as u64);

    match method {
        PackMethod::RawCopy => {}
        PackMethod::IndexedRaw => {
            out.extend_from_slice(&stream.unique_symbols);
        }
        PackMethod::IndexedCircuit => {
            let blobs = circuit_blobs.ok_or_else(|| {
                ZbitError::Internal("circuit blobs missing for indexed-circuit method".to_string())
            })?;
            if blobs.len() != stream.unique_symbols.len() {
                return Err(ZbitError::Internal(
                    "circuit blob count does not match unique symbol count".to_string(),
                ));
            }

            for (symbol, blob) in stream.unique_symbols.iter().zip(blobs.iter()) {
                out.push(*symbol);
                push_u32(&mut out, blob.len() as u32);
                out.extend_from_slice(blob);
            }
        }
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "huffman stream missing for indexed-huffman dictionary".to_string(),
                )
            })?;

            if hs.symbols.len() != hs.code_lengths.len() {
                return Err(ZbitError::Internal(
                    "huffman dictionary symbol/length mismatch".to_string(),
                ));
            }

            for (&symbol, &len) in hs.symbols.iter().zip(hs.code_lengths.iter()) {
                out.push(symbol);
                out.push(len);
            }
        }
        PackMethod::RawDeflate | PackMethod::RawZstd | PackMethod::RawXz => {}
        PackMethod::FramedRaw => {
            let run = framed_run.ok_or_else(|| {
                ZbitError::Internal("framed-run missing for framed-raw dictionary".to_string())
            })?;
            write_framed_dictionary(&mut out, run);
        }
        PackMethod::RecursiveCircuitXz => {
            let recursive = recursive_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "recursive stream missing for recursive-circuit-xz dictionary".to_string(),
                )
            })?;
            write_recursive_circuit_dictionary(&mut out, recursive);
        }
        PackMethod::MonotonicDelta => {
            let monotonic = monotonic_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "monotonic stream missing for monotonic-delta dictionary".to_string(),
                )
            })?;
            write_monotonic_delta_dictionary(&mut out, monotonic);
        }
    }

    match method {
        PackMethod::RawCopy => out.extend_from_slice(input),
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => {
            out.extend_from_slice(&stream.payload)
        }
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "huffman stream missing for indexed-huffman payload".to_string(),
                )
            })?;
            out.extend_from_slice(&hs.payload);
        }
        PackMethod::RawDeflate => {
            let payload = raw_deflate_payload.ok_or_else(|| {
                ZbitError::Internal(
                    "raw-deflate payload missing for raw-deflate method".to_string(),
                )
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::RawZstd => {
            let payload = raw_zstd_payload.ok_or_else(|| {
                ZbitError::Internal("raw-zstd payload missing for raw-zstd method".to_string())
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::RawXz => {
            let payload = raw_xz_payload.ok_or_else(|| {
                ZbitError::Internal("raw-xz payload missing for raw-xz method".to_string())
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::FramedRaw => {
            let run = framed_run.ok_or_else(|| {
                ZbitError::Internal("framed-run missing for framed-raw payload".to_string())
            })?;
            out.extend_from_slice(&run.payload);
        }
        PackMethod::RecursiveCircuitXz => {
            let recursive = recursive_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "recursive stream missing for recursive-circuit-xz payload".to_string(),
                )
            })?;
            out.extend_from_slice(&recursive.transformed_payload);
            out.extend_from_slice(&recursive.corrections_payload);
        }
        PackMethod::MonotonicDelta => {
            let monotonic = monotonic_stream.ok_or_else(|| {
                ZbitError::Internal(
                    "monotonic stream missing for monotonic-delta payload".to_string(),
                )
            })?;
            out.extend_from_slice(&monotonic.payload);
        }
    }

    Ok(out)
}

fn compress_adaptive_to_bytes(input: &[u8]) -> ZbitResult<(Vec<u8>, PackStats)> {
    let mut context = CompressionContext::new(CompressionProfile::from_env());

    let index_timer = Instant::now();
    let stream = build_index_stream(input)?;
    context.timings.index_stream_ms = index_timer.elapsed().as_secs_f64() * 1000.0;

    let raw_candidate_bytes = ZBPK_HEADER_BYTES + input.len();
    let indexed_raw_candidate_bytes =
        ZBPK_HEADER_BYTES + stream.unique_symbols.len() + stream.payload.len();

    let huffman_timer = Instant::now();
    let huffman_stream = build_huffman_stream(input, &stream)?;
    context.timings.huffman_stream_ms = huffman_timer.elapsed().as_secs_f64() * 1000.0;
    let indexed_huffman_candidate_bytes = huffman_stream
        .as_ref()
        .map(|hs| ZBPK_HEADER_BYTES + hs.symbols.len() * 2 + hs.payload.len());

    let raw_deflate_timer = Instant::now();
    let raw_deflate_payload = build_raw_deflate_payload(input)?;
    context.timings.raw_deflate_ms = raw_deflate_timer.elapsed().as_secs_f64() * 1000.0;
    let raw_deflate_candidate_bytes = Some(ZBPK_HEADER_BYTES + raw_deflate_payload.len());

    let raw_zstd_timer = Instant::now();
    let raw_zstd_payload = build_raw_zstd_payload(input)?;
    context.timings.raw_zstd_ms = raw_zstd_timer.elapsed().as_secs_f64() * 1000.0;
    let raw_zstd_candidate_bytes = Some(ZBPK_HEADER_BYTES + raw_zstd_payload.len());

    let raw_xz_timer = Instant::now();
    let raw_xz_payload = build_raw_xz_payload(input, context.profile)?;
    context.timings.raw_xz_ms = raw_xz_timer.elapsed().as_secs_f64() * 1000.0;
    let raw_xz_candidate_bytes = Some(ZBPK_HEADER_BYTES + raw_xz_payload.len());

    if !context.profile.enable_xz_extreme_for_raw_xz() {
        context.push_skipped(format!(
            "raw-xz extreme profiles skipped by '{}' profile",
            context.profile.name()
        ));
    }

    let framed_timer = Instant::now();
    let framed_run = build_framed_payload_run(input);
    context.timings.framed_extraction_ms = framed_timer.elapsed().as_secs_f64() * 1000.0;
    let framed_raw_candidate_bytes = framed_run
        .as_ref()
        .map(|run| ZBPK_HEADER_BYTES + framed_dictionary_size(run) + run.payload.len());
    let recursive_stream = match framed_run.as_ref() {
        Some(run) => build_recursive_circuit_stream(input, run, &mut context)?,
        None => {
            context.push_skipped("recursive-circuit-xz skipped: framed payload analyzer unavailable");
            None
        }
    };
    let recursive_circuit_xz_candidate_bytes = recursive_stream.as_ref().map(|recursive| {
        ZBPK_HEADER_BYTES
            + recursive_circuit_dictionary_size(recursive)
            + recursive.transformed_encoded_len
            + recursive.correction_encoded_len
    });
    let monotonic_stream = build_monotonic_delta_stream(input, &mut context)?;
    let monotonic_delta_candidate_bytes = monotonic_stream.as_ref().map(|stream| {
        ZBPK_HEADER_BYTES + monotonic_delta_dictionary_size(stream) + stream.payload.len()
    });

    let mut eval = PackEvaluation::new();
    eval.original_size = input.len();
    eval.symbol_bits = 8;
    eval.unique_symbols = stream.unique_symbols.len();
    eval.payload_bytes = stream.payload.len();
    eval.raw_total_bytes = raw_candidate_bytes;
    eval.indexed_raw_total_bytes = indexed_raw_candidate_bytes;
    eval.indexed_huffman_total_bytes = indexed_huffman_candidate_bytes;
    eval.raw_deflate_total_bytes = raw_deflate_candidate_bytes;
    eval.raw_zstd_total_bytes = raw_zstd_candidate_bytes;
    eval.raw_xz_total_bytes = raw_xz_candidate_bytes;
    eval.framed_raw_total_bytes = framed_raw_candidate_bytes;
    eval.recursive_circuit_xz_total_bytes = recursive_circuit_xz_candidate_bytes;
    eval.monotonic_delta_total_bytes = monotonic_delta_candidate_bytes;

    let (should_eval_circuit, circuit_rule_note) = should_evaluate_circuit(&eval);
    if !should_eval_circuit {
        context.push_skipped(format!("indexed-circuit skipped: {circuit_rule_note}"));
    }

    let (circuit_blobs, circuit_dictionary_bytes, indexed_circuit_candidate_bytes) =
        if should_eval_circuit {
            let (blobs, dict_bytes) = build_circuit_blobs(&stream.unique_symbols)?;
            let candidate = ZBPK_HEADER_BYTES + dict_bytes + stream.payload.len();
            eval.indexed_circuit_total_bytes = Some(candidate);
            (Some(blobs), dict_bytes, Some(candidate))
        } else {
            (None, 0usize, None)
        };

    choose_best_method(&mut eval);

    let pack_bytes = write_pack_bytes(
        eval.chosen_method,
        input,
        &stream,
        circuit_blobs.as_deref(),
        circuit_dictionary_bytes,
        huffman_stream.as_ref(),
        Some(raw_deflate_payload.as_slice()),
        Some(raw_zstd_payload.as_slice()),
        Some(raw_xz_payload.as_slice()),
        framed_run.as_ref(),
        recursive_stream.as_ref(),
        monotonic_stream.as_ref(),
    )?;

    let (bits_per_symbol, payload_bytes, huffman_dictionary_bytes) = match eval.chosen_method {
        PackMethod::RawCopy => (0u8, input.len(), 0usize),
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => {
            (stream.bits_per_symbol, stream.payload.len(), 0usize)
        }
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal("indexed-huffman selected without huffman stream".to_string())
            })?;
            (0u8, hs.payload.len(), hs.symbols.len() * 2)
        }
        PackMethod::RawDeflate => (0u8, raw_deflate_payload.len(), 0usize),
        PackMethod::RawZstd => (0u8, raw_zstd_payload.len(), 0usize),
        PackMethod::RawXz => (0u8, raw_xz_payload.len(), 0usize),
        PackMethod::FramedRaw => {
            let run = framed_run.as_ref().ok_or_else(|| {
                ZbitError::Internal("framed-raw selected without framed run".to_string())
            })?;
            (0u8, run.payload.len(), 0usize)
        }
        PackMethod::RecursiveCircuitXz => {
            let recursive = recursive_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal(
                    "recursive-circuit-xz selected without recursive stream".to_string(),
                )
            })?;
            (
                0u8,
                recursive.transformed_encoded_len + recursive.correction_encoded_len,
                0usize,
            )
        }
        PackMethod::MonotonicDelta => {
            let monotonic = monotonic_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal("monotonic-delta selected without monotonic stream".to_string())
            })?;
            (0u8, monotonic.payload.len(), 0usize)
        }
    };

    let active_profile = context.profile.name().to_string();
    let skipped_candidates = context.skipped_candidates;
    let timings = context.timings;
    let cache_stats = context.cache_stats;

    let stats = PackStats {
        original_size: input.len(),
        compressed_size: pack_bytes.len(),
        unique_symbols: stream.unique_symbols.len(),
        bits_per_symbol,
        payload_bytes,
        raw_dictionary_bytes: stream.unique_symbols.len(),
        circuit_dictionary_bytes,
        huffman_dictionary_bytes,
        raw_candidate_bytes,
        indexed_raw_candidate_bytes,
        indexed_circuit_candidate_bytes,
        indexed_huffman_candidate_bytes,
        raw_deflate_candidate_bytes,
        raw_zstd_candidate_bytes,
        raw_xz_candidate_bytes,
        framed_raw_candidate_bytes,
        recursive_circuit_xz_candidate_bytes,
        monotonic_delta_candidate_bytes,
        chosen_method: eval.chosen_method,
        chosen_reason: eval.chosen_reason,
        circuit_rule_note,
        active_profile,
        skipped_candidates,
        timings,
        cache_stats,
    };

    Ok((pack_bytes, stats))
}

pub fn compress_adaptive_to_file(input: &[u8], path: impl AsRef<Path>) -> ZbitResult<PackStats> {
    let (pack_bytes, stats) = compress_adaptive_to_bytes(input)?;
    fs::write(path.as_ref(), &pack_bytes)?;
    Ok(stats)
}

fn decode_circuit_dictionary(dict_bytes: &[u8], unique_count: usize) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;
    let mut symbols = Vec::with_capacity(unique_count);

    for _ in 0..unique_count {
        let stored_symbol = read_u8(dict_bytes, &mut cursor)?;
        let blob_len = read_u32(dict_bytes, &mut cursor)? as usize;

        let blob = dict_bytes
            .get(cursor..cursor + blob_len)
            .ok_or_else(|| ZbitError::Parse("circuit dictionary blob out of bounds".to_string()))?;
        cursor += blob_len;

        let decoded = decode_symbol_from_blob(blob)?;
        if decoded != stored_symbol {
            return Err(ZbitError::Parse(
                "decoded symbol does not match dictionary symbol".to_string(),
            ));
        }

        symbols.push(decoded);
    }

    if cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in circuit dictionary".to_string(),
        ));
    }

    Ok(symbols)
}

fn decompress_pack_bytes(bytes: &[u8]) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;

    let magic = read_u32(&bytes, &mut cursor)?;
    if magic != ZBPK_MAGIC {
        return Err(ZbitError::Parse("invalid ZBPK magic".to_string()));
    }

    let version = read_u16(&bytes, &mut cursor)?;
    if version != ZBPK_VERSION {
        return Err(ZbitError::Parse(format!(
            "unsupported ZBPK version: {version}"
        )));
    }

    let flags = read_u16(&bytes, &mut cursor)?;
    if flags != 0 {
        return Err(ZbitError::Parse(
            "non-zero flags are unsupported".to_string(),
        ));
    }

    let method = PackMethod::from_u8(read_u8(&bytes, &mut cursor)?)
        .ok_or_else(|| ZbitError::Parse("invalid pack method".to_string()))?;

    let bits_per_symbol = read_u8(&bytes, &mut cursor)?;
    let unique_count = read_u16(&bytes, &mut cursor)? as usize;
    let original_size = read_u64(&bytes, &mut cursor)? as usize;
    let dict_size = read_u64(&bytes, &mut cursor)? as usize;
    let payload_size = read_u64(&bytes, &mut cursor)? as usize;

    if original_size > ZBPK_MAX_OUTPUT_BYTES {
        return Err(ZbitError::Parse(format!(
            "original_size exceeds safety bound ({ZBPK_MAX_OUTPUT_BYTES} bytes)"
        )));
    }

    let dict = bytes
        .get(cursor..cursor + dict_size)
        .ok_or_else(|| ZbitError::Parse("dictionary range out of bounds".to_string()))?;
    cursor += dict_size;

    let payload = bytes
        .get(cursor..cursor + payload_size)
        .ok_or_else(|| ZbitError::Parse("payload range out of bounds".to_string()))?;
    cursor += payload_size;

    if cursor != bytes.len() {
        return Err(ZbitError::Parse("trailing bytes in pack".to_string()));
    }

    match method {
        PackMethod::RawCopy => {
            if bits_per_symbol != 0
                || unique_count != 0
                || dict_size != 0
                || payload_size != original_size
            {
                return Err(ZbitError::Parse(
                    "invalid raw-copy header/dictionary/payload sizing".to_string(),
                ));
            }
            return Ok(payload.to_vec());
        }
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => {
            if bits_per_symbol == 0 {
                return Err(ZbitError::Parse(
                    "indexed methods require bits_per_symbol > 0".to_string(),
                ));
            }
        }
        PackMethod::RawDeflate => {
            if bits_per_symbol != 0 || unique_count != 0 || dict_size != 0 {
                return Err(ZbitError::Parse(
                    "raw-deflate requires bits_per_symbol=0, unique_count=0, dict_size=0"
                        .to_string(),
                ));
            }
            return decode_raw_deflate_payload(payload, original_size);
        }
        PackMethod::RawZstd => {
            if bits_per_symbol != 0 || unique_count != 0 || dict_size != 0 {
                return Err(ZbitError::Parse(
                    "raw-zstd requires bits_per_symbol=0, unique_count=0, dict_size=0".to_string(),
                ));
            }
            return decode_raw_zstd_payload(payload, original_size);
        }
        PackMethod::RawXz => {
            if bits_per_symbol != 0 || unique_count != 0 || dict_size != 0 {
                return Err(ZbitError::Parse(
                    "raw-xz requires bits_per_symbol=0, unique_count=0, dict_size=0".to_string(),
                ));
            }
            return decode_raw_xz_payload(payload, original_size);
        }
        PackMethod::FramedRaw => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "framed-raw requires bits_per_symbol=0 and unique_count=0".to_string(),
                ));
            }
            return decode_framed_payload(dict, payload, original_size);
        }
        PackMethod::RecursiveCircuitXz => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "recursive-circuit-xz requires bits_per_symbol=0 and unique_count=0"
                        .to_string(),
                ));
            }
            return decode_recursive_circuit_payload(dict, payload, original_size);
        }
        PackMethod::MonotonicDelta => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "monotonic-delta requires bits_per_symbol=0 and unique_count=0".to_string(),
                ));
            }
            return decode_monotonic_delta_payload(dict, payload, original_size);
        }
        PackMethod::IndexedHuffman => {
            if bits_per_symbol != 0 {
                return Err(ZbitError::Parse(
                    "indexed-huffman requires bits_per_symbol == 0".to_string(),
                ));
            }

            let (symbols, lengths) = decode_huffman_dictionary(dict, unique_count)?;
            return decode_huffman_payload(payload, original_size, &symbols, &lengths);
        }
    }

    let symbol_by_id = match method {
        PackMethod::IndexedRaw => {
            if dict_size != unique_count {
                return Err(ZbitError::Parse(
                    "indexed-raw dictionary size must equal unique_count".to_string(),
                ));
            }
            dict.to_vec()
        }
        PackMethod::IndexedCircuit => decode_circuit_dictionary(dict, unique_count)?,
        PackMethod::RawCopy
        | PackMethod::IndexedHuffman
        | PackMethod::RawDeflate
        | PackMethod::RawZstd
        | PackMethod::RawXz
        | PackMethod::FramedRaw
        | PackMethod::RecursiveCircuitXz
        | PackMethod::MonotonicDelta => {
            unreachable!()
        }
    };

    let needed_bits = original_size * bits_per_symbol as usize;
    if payload_size * 8 < needed_bits {
        return Err(ZbitError::Parse(
            "payload does not contain enough bits for output size".to_string(),
        ));
    }

    let mut out = vec![0u8; original_size];
    let mut bit_offset = 0usize;

    for byte in out.iter_mut() {
        let idx = unpack_symbol_index(payload, bit_offset, bits_per_symbol) as usize;
        bit_offset += bits_per_symbol as usize;

        if idx >= symbol_by_id.len() {
            return Err(ZbitError::Parse(
                "symbol index out of dictionary range".to_string(),
            ));
        }

        *byte = symbol_by_id[idx];
    }

    Ok(out)
}

pub fn decompress_file(path: impl AsRef<Path>) -> ZbitResult<Vec<u8>> {
    let bytes = fs::read(path)?;
    decompress_pack_bytes(&bytes)
}

fn usize_to_u32(value: usize, what: &str) -> ZbitResult<u32> {
    u32::try_from(value)
        .map_err(|_| ZbitError::Limit(format!("{what} exceeds u32::MAX in stream format")))
}

fn usize_to_u64(value: usize, what: &str) -> ZbitResult<u64> {
    u64::try_from(value)
        .map_err(|_| ZbitError::Limit(format!("{what} exceeds u64::MAX in stream format")))
}

fn u64_to_usize(value: u64, what: &str) -> ZbitResult<usize> {
    usize::try_from(value)
        .map_err(|_| ZbitError::Parse(format!("{what} exceeds platform usize in stream format")))
}

fn validate_stream_options(options: &StreamPackOptions) -> ZbitResult<()> {
    if options.chunk_size == 0 {
        return Err(ZbitError::InvalidArg(
            "stream chunk_size must be greater than 0",
        ));
    }
    if options.key_piece_interval == 0 {
        return Err(ZbitError::InvalidArg(
            "stream key_piece_interval must be greater than 0",
        ));
    }
    if options.max_group_pieces == 0 {
        return Err(ZbitError::InvalidArg(
            "stream max_group_pieces must be greater than 0",
        ));
    }
    if options.max_group_depth > 32 {
        return Err(ZbitError::InvalidArg(
            "stream max_group_depth must be <= 32",
        ));
    }
    let _ = usize_to_u32(options.chunk_size, "stream chunk_size")?;
    let _ = usize_to_u32(options.key_piece_interval, "stream key_piece_interval")?;
    let _ = usize_to_u32(options.max_group_pieces, "stream max_group_pieces")?;
    Ok(())
}

fn expected_chunk_count(original_size: usize, chunk_size: usize) -> usize {
    if original_size == 0 {
        0
    } else {
        (original_size + chunk_size - 1) / chunk_size
    }
}

fn expected_stream_block_len(
    original_size: usize,
    chunk_size: usize,
    total_chunks: usize,
    first_chunk_index: usize,
    block_chunk_count: usize,
) -> ZbitResult<usize> {
    if block_chunk_count == 0 {
        return Ok(0);
    }

    let block_end = first_chunk_index
        .checked_add(block_chunk_count)
        .ok_or_else(|| ZbitError::Parse("stream block end overflow".to_string()))?;
    if block_end > total_chunks {
        return Err(ZbitError::Parse(
            "stream block exceeds total chunk count".to_string(),
        ));
    }

    if total_chunks == 0 {
        return Ok(0);
    }

    let last_chunk_len = original_size
        .checked_sub(
            chunk_size
                .checked_mul(total_chunks.saturating_sub(1))
                .ok_or_else(|| ZbitError::Parse("stream chunk geometry overflow".to_string()))?,
        )
        .ok_or_else(|| ZbitError::Parse("stream chunk geometry underflow".to_string()))?;

    if block_end < total_chunks {
        return chunk_size
            .checked_mul(block_chunk_count)
            .ok_or_else(|| ZbitError::Parse("stream block size overflow".to_string()));
    }

    if block_chunk_count == 1 {
        return Ok(last_chunk_len);
    }

    let full_chunks_in_block = block_chunk_count - 1;
    let full_len = chunk_size
        .checked_mul(full_chunks_in_block)
        .ok_or_else(|| ZbitError::Parse("stream final block size overflow".to_string()))?;
    full_len
        .checked_add(last_chunk_len)
        .ok_or_else(|| ZbitError::Parse("stream final block size overflow".to_string()))
}

fn split_stream_chunks(input: &[u8], chunk_size: usize) -> Vec<Vec<u8>> {
    if input.is_empty() {
        return Vec::new();
    }
    input
        .chunks(chunk_size)
        .map(|chunk| chunk.to_vec())
        .collect()
}

fn compress_stream_realtime_pack_bytes(
    input: &[u8],
    allow_recursive_candidate: bool,
    context: &mut CompressionContext,
) -> ZbitResult<(Vec<u8>, PackMethod)> {
    let raw_deflate_payload = build_raw_deflate_payload(input)?;
    let raw_zstd_payload = build_raw_zstd_payload(input)?;

    let empty_stream = IndexStream {
        unique_symbols: Vec::new(),
        bits_per_symbol: 0,
        payload: Vec::new(),
        frequencies: [0u32; 256],
    };

    let raw_copy_bytes = write_pack_bytes(
        PackMethod::RawCopy,
        input,
        &empty_stream,
        None,
        0,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;

    let raw_deflate_bytes = write_pack_bytes(
        PackMethod::RawDeflate,
        input,
        &empty_stream,
        None,
        0,
        None,
        Some(raw_deflate_payload.as_slice()),
        None,
        None,
        None,
        None,
        None,
    )?;

    let raw_zstd_bytes = write_pack_bytes(
        PackMethod::RawZstd,
        input,
        &empty_stream,
        None,
        0,
        None,
        None,
        Some(raw_zstd_payload.as_slice()),
        None,
        None,
        None,
        None,
    )?;

    let mut candidates = vec![
        (PackMethod::RawCopy, raw_copy_bytes),
        (PackMethod::RawDeflate, raw_deflate_bytes),
        (PackMethod::RawZstd, raw_zstd_bytes),
    ];

    if let Some(framed_run) = build_framed_payload_run(input) {
        let framed_bytes = write_pack_bytes(
            PackMethod::FramedRaw,
            input,
            &empty_stream,
            None,
            0,
            None,
            None,
            None,
            None,
            Some(&framed_run),
            None,
            None,
        )?;
        candidates.push((PackMethod::FramedRaw, framed_bytes));

        if allow_recursive_candidate
            && context.profile.should_attempt_recursive_on_realtime_blocks()
            && input.len() >= (512 * 1024)
        {
            if let Some(recursive_stream) =
                build_recursive_circuit_stream(input, &framed_run, context)?
            {
                let recursive_bytes = write_pack_bytes(
                    PackMethod::RecursiveCircuitXz,
                    input,
                    &empty_stream,
                    None,
                    0,
                    None,
                    None,
                    None,
                    None,
                    None,
                    Some(&recursive_stream),
                    None,
                )?;
                let validation_timer = Instant::now();
                if let Ok(decoded) = decompress_pack_bytes(&recursive_bytes) {
                    if decoded == input {
                        candidates.push((PackMethod::RecursiveCircuitXz, recursive_bytes));
                    }
                }
                context.timings.candidate_validation_ms +=
                    validation_timer.elapsed().as_secs_f64() * 1000.0;
            }
        } else if allow_recursive_candidate
            && !context.profile.should_attempt_recursive_on_realtime_blocks()
        {
            context.push_skipped(format!(
                "stream recursive realtime candidate skipped by '{}' profile",
                context.profile.name()
            ));
        }
    }

    candidates.sort_by_key(|(_, bytes)| bytes.len());
    let (method, bytes) = candidates
        .into_iter()
        .next()
        .ok_or_else(|| ZbitError::Internal("missing stream realtime pack candidate".to_string()))?;
    Ok((bytes, method))
}

fn compress_stream_wide_overfit_pack_bytes(
    input: &[u8],
    context: &mut CompressionContext,
) -> ZbitResult<(Vec<u8>, PackMethod)> {
    let raw_deflate_payload = build_raw_deflate_payload(input)?;
    let raw_zstd_payload = build_raw_zstd_payload(input)?;

    let empty_stream = IndexStream {
        unique_symbols: Vec::new(),
        bits_per_symbol: 0,
        payload: Vec::new(),
        frequencies: [0u32; 256],
    };

    let raw_copy_bytes = write_pack_bytes(
        PackMethod::RawCopy,
        input,
        &empty_stream,
        None,
        0,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )?;
    let raw_deflate_bytes = write_pack_bytes(
        PackMethod::RawDeflate,
        input,
        &empty_stream,
        None,
        0,
        None,
        Some(raw_deflate_payload.as_slice()),
        None,
        None,
        None,
        None,
        None,
    )?;
    let raw_zstd_bytes = write_pack_bytes(
        PackMethod::RawZstd,
        input,
        &empty_stream,
        None,
        0,
        None,
        None,
        Some(raw_zstd_payload.as_slice()),
        None,
        None,
        None,
        None,
    )?;

    let mut candidates = vec![
        (PackMethod::RawCopy, raw_copy_bytes),
        (PackMethod::RawDeflate, raw_deflate_bytes),
        (PackMethod::RawZstd, raw_zstd_bytes),
    ];

    if let Some(framed_run) = build_framed_payload_run(input) {
        let framed_bytes = write_pack_bytes(
            PackMethod::FramedRaw,
            input,
            &empty_stream,
            None,
            0,
            None,
            None,
            None,
            None,
            Some(&framed_run),
            None,
            None,
        )?;
        candidates.push((PackMethod::FramedRaw, framed_bytes));

        if let Some(recursive_stream) = build_recursive_circuit_stream(input, &framed_run, context)?
        {
            let recursive_bytes = write_pack_bytes(
                PackMethod::RecursiveCircuitXz,
                input,
                &empty_stream,
                None,
                0,
                None,
                None,
                None,
                None,
                None,
                Some(&recursive_stream),
                None,
            )?;
            let validation_timer = Instant::now();
            if let Ok(decoded) = decompress_pack_bytes(&recursive_bytes) {
                if decoded == input {
                    candidates.push((PackMethod::RecursiveCircuitXz, recursive_bytes));
                }
            }
            context.timings.candidate_validation_ms +=
                validation_timer.elapsed().as_secs_f64() * 1000.0;
        }
    }

    candidates.sort_by_key(|(_, bytes)| bytes.len());
    let (method, bytes) = candidates.into_iter().next().ok_or_else(|| {
        ZbitError::Internal("missing stream wide-overfit pack candidate".to_string())
    })?;
    Ok((bytes, method))
}

fn compress_stream_range_pack_bytes(
    input: &[u8],
    realtime_mode: bool,
    allow_recursive_candidate: bool,
    context: &mut CompressionContext,
) -> ZbitResult<(Vec<u8>, PackMethod)> {
    if realtime_mode {
        compress_stream_realtime_pack_bytes(input, allow_recursive_candidate, context)
    } else {
        let (pack_bytes, stats) = compress_adaptive_to_bytes(input)?;
        Ok((pack_bytes, stats.chosen_method))
    }
}

fn stream_pack_range_candidate(
    input_hash: PayloadHash,
    chunks: &[Vec<u8>],
    absolute_chunk_base: usize,
    start: usize,
    end: usize,
    realtime_mode: bool,
    enable_realtime_root_recursive_candidate: bool,
    context: &mut CompressionContext,
) -> ZbitResult<PackedRangeCandidate> {
    let abs_start_chunk = absolute_chunk_base
        .checked_add(start)
        .ok_or_else(|| ZbitError::Limit("stream range absolute start overflow".to_string()))?;
    let abs_end_chunk = absolute_chunk_base
        .checked_add(end)
        .ok_or_else(|| ZbitError::Limit("stream range absolute end overflow".to_string()))?;
    let original_size = chunks[start..end]
        .iter()
        .map(|chunk| chunk.len())
        .sum::<usize>();
    let is_block_root = start == 0 && end == chunks.len();
    let allow_recursive_candidate = enable_realtime_root_recursive_candidate
        && is_block_root
        && original_size >= (512 * 1024);
    let cache_key = StreamRangeCacheKey {
        input_hash,
        abs_start_chunk,
        abs_end_chunk,
        realtime_mode,
        allow_recursive_candidate,
        profile: context.profile,
    };
    if let Some(cached) = context.cache.stream_range_candidates.get(&cache_key) {
        context.cache_stats.stream_range_hits =
            context.cache_stats.stream_range_hits.saturating_add(1);
        return Ok(cached.clone());
    }
    context.cache_stats.stream_range_misses = context.cache_stats.stream_range_misses.saturating_add(1);

    let mut merged = Vec::with_capacity(original_size);
    for chunk in &chunks[start..end] {
        merged.extend_from_slice(chunk);
    }

    let (pack_bytes, method) = compress_stream_range_pack_bytes(
        &merged,
        realtime_mode,
        allow_recursive_candidate,
        context,
    )?;
    let candidate = PackedRangeCandidate {
        pack_bytes,
        method,
        original_size,
    };
    context
        .cache
        .stream_range_candidates
        .insert(cache_key, candidate.clone());
    Ok(candidate)
}

fn stream_piece_node(
    chunk_len: usize,
    method: PackMethod,
    pack_bytes: Vec<u8>,
) -> ZbitResult<StreamNode> {
    let encoded_size = 1usize
        .checked_add(4)
        .and_then(|v| v.checked_add(1))
        .and_then(|v| v.checked_add(4))
        .and_then(|v| v.checked_add(pack_bytes.len()))
        .ok_or_else(|| ZbitError::Limit("stream piece node size overflow".to_string()))?;

    Ok(StreamNode {
        kind: StreamNodeKind::Piece {
            chunk_len,
            method,
            pack_bytes,
        },
        chunk_count: 1,
        original_len: chunk_len,
        encoded_size,
    })
}

fn stream_group_node(
    chunk_count: usize,
    original_len: usize,
    method: PackMethod,
    pack_bytes: Vec<u8>,
) -> ZbitResult<StreamNode> {
    let encoded_size = 1usize
        .checked_add(4)
        .and_then(|v| v.checked_add(8))
        .and_then(|v| v.checked_add(1))
        .and_then(|v| v.checked_add(4))
        .and_then(|v| v.checked_add(pack_bytes.len()))
        .ok_or_else(|| ZbitError::Limit("stream group node size overflow".to_string()))?;

    Ok(StreamNode {
        kind: StreamNodeKind::Group {
            chunk_count,
            original_len,
            method,
            pack_bytes,
        },
        chunk_count,
        original_len,
        encoded_size,
    })
}

fn stream_global_slice_node(
    chunk_count: usize,
    original_offset: usize,
    original_len: usize,
) -> ZbitResult<StreamNode> {
    let encoded_size = 1usize
        .checked_add(4)
        .and_then(|v| v.checked_add(8))
        .and_then(|v| v.checked_add(8))
        .ok_or_else(|| ZbitError::Limit("stream global-slice node size overflow".to_string()))?;

    Ok(StreamNode {
        kind: StreamNodeKind::GlobalSlice {
            chunk_count,
            original_offset,
            original_len,
        },
        chunk_count,
        original_len,
        encoded_size,
    })
}

fn stream_split_node(level: u8, left: StreamNode, right: StreamNode) -> ZbitResult<StreamNode> {
    let chunk_count = left
        .chunk_count
        .checked_add(right.chunk_count)
        .ok_or_else(|| ZbitError::Limit("stream split chunk count overflow".to_string()))?;
    let original_len = left
        .original_len
        .checked_add(right.original_len)
        .ok_or_else(|| ZbitError::Limit("stream split length overflow".to_string()))?;
    let encoded_size = 1usize
        .checked_add(1)
        .and_then(|v| v.checked_add(left.encoded_size))
        .and_then(|v| v.checked_add(right.encoded_size))
        .ok_or_else(|| ZbitError::Limit("stream split node size overflow".to_string()))?;

    Ok(StreamNode {
        kind: StreamNodeKind::Split {
            level,
            left: Box::new(left),
            right: Box::new(right),
        },
        chunk_count,
        original_len,
        encoded_size,
    })
}

fn build_best_stream_node(
    chunks: &[Vec<u8>],
    start: usize,
    end: usize,
    absolute_chunk_base: usize,
    depth_remaining: u8,
    max_group_pieces: usize,
    realtime_mode: bool,
    enable_realtime_root_recursive_candidate: bool,
    input_hash: PayloadHash,
    context: &mut CompressionContext,
    memo: &mut HashMap<(usize, usize, u8), StreamNode>,
) -> ZbitResult<StreamNode> {
    if let Some(cached) = memo.get(&(start, end, depth_remaining)) {
        return Ok(cached.clone());
    }

    let chunk_span = end.saturating_sub(start);
    if chunk_span == 0 {
        return Err(ZbitError::Internal(
            "stream node builder called with empty range".to_string(),
        ));
    }

    if chunk_span == 1 {
        let candidate = stream_pack_range_candidate(
            input_hash,
            chunks,
            absolute_chunk_base,
            start,
            end,
            realtime_mode,
            enable_realtime_root_recursive_candidate,
            context,
        )?;
        let node = stream_piece_node(
            chunks[start].len(),
            candidate.method,
            candidate.pack_bytes.clone(),
        )?;
        memo.insert((start, end, depth_remaining), node.clone());
        return Ok(node);
    }

    let child_depth = if depth_remaining > 0 {
        depth_remaining - 1
    } else {
        0
    };

    let mut best_split: Option<StreamNode> = None;
    for mid in (start + 1)..end {
        let left = build_best_stream_node(
            chunks,
            start,
            mid,
            absolute_chunk_base,
            child_depth,
            max_group_pieces,
            realtime_mode,
            enable_realtime_root_recursive_candidate,
            input_hash,
            context,
            memo,
        )?;
        let right = build_best_stream_node(
            chunks,
            mid,
            end,
            absolute_chunk_base,
            child_depth,
            max_group_pieces,
            realtime_mode,
            enable_realtime_root_recursive_candidate,
            input_hash,
            context,
            memo,
        )?;
        let split = stream_split_node(depth_remaining, left, right)?;
        match &best_split {
            Some(best) if split.encoded_size >= best.encoded_size => {}
            _ => best_split = Some(split),
        }
    }

    let mut best = best_split
        .ok_or_else(|| ZbitError::Internal("failed to build stream split candidate".to_string()))?;

    let is_block_root = start == 0 && end == chunks.len();
    let allow_group_candidate = depth_remaining > 0
        && chunk_span <= max_group_pieces
        && (chunk_span <= 2 || is_block_root || chunk_span == max_group_pieces);
    if allow_group_candidate {
        let grouped_candidate = stream_pack_range_candidate(
            input_hash,
            chunks,
            absolute_chunk_base,
            start,
            end,
            realtime_mode,
            enable_realtime_root_recursive_candidate,
            context,
        )?;
        let grouped = stream_group_node(
            chunk_span,
            grouped_candidate.original_size,
            grouped_candidate.method,
            grouped_candidate.pack_bytes.clone(),
        )?;
        if grouped.encoded_size <= best.encoded_size {
            best = grouped;
        }
    }

    memo.insert((start, end, depth_remaining), best.clone());
    Ok(best)
}

fn collect_stream_node_counts(node: &StreamNode, depth: u8, counts: &mut StreamNodeCounts) {
    counts.max_depth_used = counts.max_depth_used.max(depth);
    match &node.kind {
        StreamNodeKind::Piece { .. } => counts.piece_nodes += 1,
        StreamNodeKind::Group { .. } => counts.grouped_nodes += 1,
        StreamNodeKind::GlobalSlice { .. } => counts.grouped_nodes += 1,
        StreamNodeKind::Split { left, right, .. } => {
            counts.split_nodes += 1;
            collect_stream_node_counts(left, depth.saturating_add(1), counts);
            collect_stream_node_counts(right, depth.saturating_add(1), counts);
        }
    }
}

fn stream_node_history_method(node: &StreamNode) -> PackMethod {
    match &node.kind {
        StreamNodeKind::Piece { method, .. } | StreamNodeKind::Group { method, .. } => *method,
        StreamNodeKind::GlobalSlice { .. } => PackMethod::RecursiveCircuitXz,
        StreamNodeKind::Split { left, right, .. } => {
            if left.original_len >= right.original_len {
                stream_node_history_method(left)
            } else {
                stream_node_history_method(right)
            }
        }
    }
}

fn write_stream_node(out: &mut Vec<u8>, node: &StreamNode) -> ZbitResult<()> {
    match &node.kind {
        StreamNodeKind::Piece {
            chunk_len,
            method,
            pack_bytes,
        } => {
            out.push(ZBPS_NODE_KIND_PIECE);
            push_u32(out, usize_to_u32(*chunk_len, "stream chunk_len")?);
            out.push(method.as_u8());
            push_u32(
                out,
                usize_to_u32(pack_bytes.len(), "stream piece pack bytes")?,
            );
            out.extend_from_slice(pack_bytes);
        }
        StreamNodeKind::Group {
            chunk_count,
            original_len,
            method,
            pack_bytes,
        } => {
            out.push(ZBPS_NODE_KIND_GROUP);
            push_u32(
                out,
                usize_to_u32(*chunk_count, "stream grouped chunk_count")?,
            );
            push_u64(
                out,
                usize_to_u64(*original_len, "stream grouped original_len")?,
            );
            out.push(method.as_u8());
            push_u32(
                out,
                usize_to_u32(pack_bytes.len(), "stream group pack bytes")?,
            );
            out.extend_from_slice(pack_bytes);
        }
        StreamNodeKind::Split { level, left, right } => {
            out.push(ZBPS_NODE_KIND_SPLIT);
            out.push(*level);
            write_stream_node(out, left)?;
            write_stream_node(out, right)?;
        }
        StreamNodeKind::GlobalSlice {
            chunk_count,
            original_offset,
            original_len,
        } => {
            out.push(ZBPS_NODE_KIND_GLOBAL_SLICE);
            push_u32(
                out,
                usize_to_u32(*chunk_count, "stream global-slice chunk_count")?,
            );
            push_u64(
                out,
                usize_to_u64(*original_offset, "stream global-slice original_offset")?,
            );
            push_u64(
                out,
                usize_to_u64(*original_len, "stream global-slice original_len")?,
            );
        }
    }
    Ok(())
}

fn decode_stream_node(
    node_bytes: &[u8],
    cursor: &mut usize,
    global_output: Option<&[u8]>,
) -> ZbitResult<StreamDecodedNode> {
    let kind = read_u8(node_bytes, cursor)?;
    match kind {
        ZBPS_NODE_KIND_PIECE => {
            let chunk_len = read_u32(node_bytes, cursor)? as usize;
            let expected_method = PackMethod::from_u8(read_u8(node_bytes, cursor)?)
                .ok_or_else(|| ZbitError::Parse("invalid stream piece method id".to_string()))?;
            let pack_len = read_u32(node_bytes, cursor)? as usize;
            let pack_bytes = node_bytes.get(*cursor..*cursor + pack_len).ok_or_else(|| {
                ZbitError::Parse("stream piece pack range out of bounds".to_string())
            })?;
            *cursor += pack_len;

            let output = decompress_pack_bytes(pack_bytes)?;
            if output.len() != chunk_len {
                return Err(ZbitError::Parse(format!(
                    "stream piece output length mismatch: expected {chunk_len}, got {}",
                    output.len()
                )));
            }

            let actual_method = PackMethod::from_u8(*pack_bytes.get(8).ok_or_else(|| {
                ZbitError::Parse("stream piece packed header too short".to_string())
            })?)
            .ok_or_else(|| ZbitError::Parse("stream piece packed method id invalid".to_string()))?;
            if actual_method != expected_method {
                return Err(ZbitError::Parse(format!(
                    "stream piece method mismatch: expected {}, got {}",
                    expected_method.name(),
                    actual_method.name()
                )));
            }

            Ok(StreamDecodedNode {
                bytes: output,
                chunk_count: 1,
            })
        }
        ZBPS_NODE_KIND_GROUP => {
            let chunk_count = read_u32(node_bytes, cursor)? as usize;
            let original_len =
                u64_to_usize(read_u64(node_bytes, cursor)?, "stream grouped original_len")?;
            let expected_method = PackMethod::from_u8(read_u8(node_bytes, cursor)?)
                .ok_or_else(|| ZbitError::Parse("invalid stream group method id".to_string()))?;
            let pack_len = read_u32(node_bytes, cursor)? as usize;
            let pack_bytes = node_bytes.get(*cursor..*cursor + pack_len).ok_or_else(|| {
                ZbitError::Parse("stream group pack range out of bounds".to_string())
            })?;
            *cursor += pack_len;

            let output = decompress_pack_bytes(pack_bytes)?;
            if output.len() != original_len {
                return Err(ZbitError::Parse(format!(
                    "stream group output length mismatch: expected {original_len}, got {}",
                    output.len()
                )));
            }

            let actual_method = PackMethod::from_u8(*pack_bytes.get(8).ok_or_else(|| {
                ZbitError::Parse("stream group packed header too short".to_string())
            })?)
            .ok_or_else(|| ZbitError::Parse("stream group packed method id invalid".to_string()))?;
            if actual_method != expected_method {
                return Err(ZbitError::Parse(format!(
                    "stream group method mismatch: expected {}, got {}",
                    expected_method.name(),
                    actual_method.name()
                )));
            }

            Ok(StreamDecodedNode {
                bytes: output,
                chunk_count,
            })
        }
        ZBPS_NODE_KIND_SPLIT => {
            let _level = read_u8(node_bytes, cursor)?;
            let left = decode_stream_node(node_bytes, cursor, global_output)?;
            let right = decode_stream_node(node_bytes, cursor, global_output)?;

            let mut bytes = Vec::with_capacity(
                left.bytes
                    .len()
                    .checked_add(right.bytes.len())
                    .ok_or_else(|| ZbitError::Limit("stream split output overflow".to_string()))?,
            );
            bytes.extend_from_slice(&left.bytes);
            bytes.extend_from_slice(&right.bytes);

            let chunk_count = left
                .chunk_count
                .checked_add(right.chunk_count)
                .ok_or_else(|| ZbitError::Limit("stream split chunk count overflow".to_string()))?;

            Ok(StreamDecodedNode { bytes, chunk_count })
        }
        ZBPS_NODE_KIND_GLOBAL_SLICE => {
            let chunk_count = read_u32(node_bytes, cursor)? as usize;
            let original_offset = u64_to_usize(
                read_u64(node_bytes, cursor)?,
                "stream global-slice original_offset",
            )?;
            let original_len = u64_to_usize(
                read_u64(node_bytes, cursor)?,
                "stream global-slice original_len",
            )?;

            let global = global_output.ok_or_else(|| {
                ZbitError::Parse(
                    "stream global-slice node missing global overfit payload".to_string(),
                )
            })?;

            let range_end = original_offset.checked_add(original_len).ok_or_else(|| {
                ZbitError::Parse("stream global-slice range overflow".to_string())
            })?;
            let bytes = global
                .get(original_offset..range_end)
                .ok_or_else(|| {
                    ZbitError::Parse("stream global-slice range out of bounds".to_string())
                })?
                .to_vec();

            Ok(StreamDecodedNode { bytes, chunk_count })
        }
        _ => Err(ZbitError::Parse(format!(
            "invalid stream node kind: {kind}"
        ))),
    }
}

fn parse_stream_header(bytes: &[u8], cursor: &mut usize) -> ZbitResult<StreamHeader> {
    let magic = read_u32(bytes, cursor)?;
    if magic != ZBPS_MAGIC {
        return Err(ZbitError::Parse("invalid ZBPS magic".to_string()));
    }

    let version = read_u16(bytes, cursor)?;
    if version != ZBPS_VERSION {
        return Err(ZbitError::Parse(format!(
            "unsupported ZBPS version: {version}"
        )));
    }

    let flags = read_u16(bytes, cursor)?;
    if flags
        & !(ZBPS_FLAG_CARRY_GROUPING_HISTORY
            | ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS
            | ZBPS_FLAG_SHARED_GROUPING_PAYLOAD)
        != 0
    {
        return Err(ZbitError::Parse(format!(
            "unsupported ZBPS flags: 0x{flags:04x}"
        )));
    }

    let chunk_size = read_u32(bytes, cursor)? as usize;
    let key_piece_interval = read_u32(bytes, cursor)? as usize;
    let _max_group_depth = read_u8(bytes, cursor)?;
    let _reserved0 = read_u8(bytes, cursor)?;
    let _reserved1 = read_u8(bytes, cursor)?;
    let _reserved2 = read_u8(bytes, cursor)?;
    let max_group_pieces = read_u32(bytes, cursor)? as usize;
    let original_size = u64_to_usize(read_u64(bytes, cursor)?, "stream original_size")?;
    let total_chunks = read_u32(bytes, cursor)? as usize;
    let block_count = read_u32(bytes, cursor)? as usize;

    if chunk_size == 0 {
        return Err(ZbitError::Parse(
            "stream chunk_size must be > 0".to_string(),
        ));
    }
    if key_piece_interval == 0 {
        return Err(ZbitError::Parse(
            "stream key_piece_interval must be > 0".to_string(),
        ));
    }
    if max_group_pieces == 0 {
        return Err(ZbitError::Parse(
            "stream max_group_pieces must be > 0".to_string(),
        ));
    }
    if original_size > ZBPK_MAX_OUTPUT_BYTES {
        return Err(ZbitError::Parse(format!(
            "stream original_size exceeds safety bound ({ZBPK_MAX_OUTPUT_BYTES} bytes)"
        )));
    }

    let expected_chunks = expected_chunk_count(original_size, chunk_size);
    if expected_chunks != total_chunks {
        return Err(ZbitError::Parse(format!(
            "stream total_chunks mismatch: header={total_chunks}, expected={expected_chunks}"
        )));
    }
    let expected_blocks = if total_chunks == 0 {
        0
    } else {
        (total_chunks + key_piece_interval - 1) / key_piece_interval
    };
    if expected_blocks != block_count {
        return Err(ZbitError::Parse(format!(
            "stream block_count mismatch: header={block_count}, expected={expected_blocks}"
        )));
    }

    Ok(StreamHeader {
        flags,
        chunk_size,
        key_piece_interval,
        original_size,
        total_chunks,
        block_count,
    })
}

fn compress_stream_to_bytes(
    input: &[u8],
    options: &StreamPackOptions,
) -> ZbitResult<(Vec<u8>, StreamPackStats)> {
    validate_stream_options(options)?;
    let mut context = CompressionContext::new(CompressionProfile::from_env());

    let chunks = split_stream_chunks(input, options.chunk_size);
    let total_chunks = chunks.len();
    let block_count = if total_chunks == 0 {
        0
    } else {
        (total_chunks + options.key_piece_interval - 1) / options.key_piece_interval
    };

    let mut flags = 0u16;
    if options.carry_grouping_history {
        flags |= ZBPS_FLAG_CARRY_GROUPING_HISTORY;
    }
    if options.wide_overfitting_circuits {
        flags |= ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS;
    }
    let input_hash = payload_hash(input);

    let shared_grouping_payload_bytes = if !options.wide_overfitting_circuits
        && options.realtime_mode
        && block_count > 1
        && input.len() >= (1024 * 1024)
    {
        let shared_timer = Instant::now();
        let (pack_bytes, _) = compress_stream_wide_overfit_pack_bytes(input, &mut context)?;
        match decompress_pack_bytes(&pack_bytes) {
            Ok(decoded) if decoded == input => {
                flags |= ZBPS_FLAG_SHARED_GROUPING_PAYLOAD;
                context.timings.stream_global_payload_ms +=
                    shared_timer.elapsed().as_secs_f64() * 1000.0;
                Some(pack_bytes)
            }
            _ => {
                context.push_skipped(
                    "shared grouping payload rejected: roundtrip validation failed",
                );
                context.timings.stream_global_payload_ms +=
                    shared_timer.elapsed().as_secs_f64() * 1000.0;
                None
            }
        }
    } else {
        None
    };

    if options.wide_overfitting_circuits {
        let global_timer = Instant::now();
        let (global_pack_bytes, global_method) =
            compress_stream_wide_overfit_pack_bytes(input, &mut context)?;
        context.timings.stream_global_payload_ms += global_timer.elapsed().as_secs_f64() * 1000.0;
        let mut out = Vec::with_capacity(
            ZBPS_HEADER_BYTES
                .checked_add(4)
                .and_then(|v| v.checked_add(global_pack_bytes.len()))
                .and_then(|v| {
                    v.checked_add(
                        block_count
                            .checked_mul(
                                ZBPS_BLOCK_HEADER_BYTES
                                    .checked_add(1 + 4 + 8 + 8)
                                    .unwrap_or(0),
                            )
                            .unwrap_or(0),
                    )
                })
                .ok_or_else(|| {
                    ZbitError::Limit("stream wide-overfit output size overflow".to_string())
                })?,
        );

        push_u32(&mut out, ZBPS_MAGIC);
        push_u16(&mut out, ZBPS_VERSION);
        push_u16(&mut out, flags);
        push_u32(
            &mut out,
            usize_to_u32(options.chunk_size, "stream chunk_size")?,
        );
        push_u32(
            &mut out,
            usize_to_u32(options.key_piece_interval, "stream key_piece_interval")?,
        );
        out.push(options.max_group_depth);
        out.extend_from_slice(&[0u8; 3]);
        push_u32(
            &mut out,
            usize_to_u32(options.max_group_pieces, "stream max_group_pieces")?,
        );
        push_u64(
            &mut out,
            usize_to_u64(input.len(), "stream original input size")?,
        );
        push_u32(&mut out, usize_to_u32(total_chunks, "stream total_chunks")?);
        push_u32(&mut out, usize_to_u32(block_count, "stream block_count")?);

        push_u32(
            &mut out,
            usize_to_u32(
                global_pack_bytes.len(),
                "stream wide-overfit global pack bytes length",
            )?,
        );
        out.extend_from_slice(&global_pack_bytes);

        let mut counts = StreamNodeCounts::default();
        let mut grouping_hint_updates = 0usize;
        let mut history_method: Option<PackMethod> = None;

        for block_index in 0..block_count {
            let block_start = block_index * options.key_piece_interval;
            let block_end = (block_start + options.key_piece_interval).min(total_chunks);
            let block_chunk_count = block_end - block_start;
            let block_original_len = expected_stream_block_len(
                input.len(),
                options.chunk_size,
                total_chunks,
                block_start,
                block_chunk_count,
            )?;
            let block_offset = block_start.checked_mul(options.chunk_size).ok_or_else(|| {
                ZbitError::Limit("stream wide-overfit block offset overflow".to_string())
            })?;

            let root =
                stream_global_slice_node(block_chunk_count, block_offset, block_original_len)?;
            collect_stream_node_counts(&root, 0, &mut counts);

            let mut node_bytes = Vec::with_capacity(root.encoded_size);
            write_stream_node(&mut node_bytes, &root)?;
            if node_bytes.len() != root.encoded_size {
                return Err(ZbitError::Internal(format!(
                    "stream wide-overfit node encoded size mismatch: expected {}, got {}",
                    root.encoded_size,
                    node_bytes.len()
                )));
            }

            push_u32(
                &mut out,
                usize_to_u32(block_start, "stream block first_chunk_index")?,
            );
            push_u32(
                &mut out,
                usize_to_u32(block_chunk_count, "stream block chunk_count")?,
            );
            push_u64(
                &mut out,
                usize_to_u64(block_original_len, "stream block original_len")?,
            );
            let history_field = if options.carry_grouping_history {
                history_method
                    .map(|method| method.as_u8())
                    .unwrap_or(ZBPS_HISTORY_NONE)
            } else {
                ZBPS_HISTORY_NONE
            };
            out.push(history_field);
            if history_field != ZBPS_HISTORY_NONE {
                grouping_hint_updates = grouping_hint_updates.saturating_add(1);
            }
            push_u32(
                &mut out,
                usize_to_u32(node_bytes.len(), "stream block node bytes")?,
            );
            out.extend_from_slice(&node_bytes);

            history_method = Some(global_method);
        }

        let stats = StreamPackStats {
            original_size: input.len(),
            compressed_size: out.len(),
            chunk_size: options.chunk_size,
            total_chunks,
            key_piece_interval: options.key_piece_interval,
            key_piece_count: block_count,
            block_count,
            max_group_depth: options.max_group_depth,
            max_group_pieces: options.max_group_pieces,
            piece_node_count: counts.piece_nodes,
            grouped_node_count: counts.grouped_nodes,
            split_node_count: counts.split_nodes,
            max_depth_used: counts.max_depth_used,
            grouping_hint_updates,
            key_piece_decode_note: format!(
                "receiver can start from chunk indices that are multiples of {} (block/key boundaries)",
                options.key_piece_interval
            ),
            effective_wide_overfitting_circuits: true,
            adaptive_wide_promotion_used: false,
            shared_grouping_payload_used: false,
            active_profile: context.profile.name().to_string(),
            skipped_candidates: context.skipped_candidates,
            timings: context.timings,
            cache_stats: context.cache_stats,
        };

        return Ok((out, stats));
    }

    let shared_payload_overhead = shared_grouping_payload_bytes
        .as_ref()
        .map(|payload| {
            4usize
                .checked_add(payload.len())
                .ok_or_else(|| ZbitError::Limit("stream shared payload size overflow".to_string()))
        })
        .transpose()?
        .unwrap_or(0);
    let mut out = Vec::with_capacity(
        ZBPS_HEADER_BYTES
            .checked_add(shared_payload_overhead)
            .and_then(|v| v.checked_add(block_count.checked_mul(ZBPS_BLOCK_HEADER_BYTES)?))
            .ok_or_else(|| ZbitError::Limit("stream output header size overflow".to_string()))?,
    );

    push_u32(&mut out, ZBPS_MAGIC);
    push_u16(&mut out, ZBPS_VERSION);
    push_u16(&mut out, flags);
    push_u32(
        &mut out,
        usize_to_u32(options.chunk_size, "stream chunk_size")?,
    );
    push_u32(
        &mut out,
        usize_to_u32(options.key_piece_interval, "stream key_piece_interval")?,
    );
    out.push(options.max_group_depth);
    out.extend_from_slice(&[0u8; 3]);
    push_u32(
        &mut out,
        usize_to_u32(options.max_group_pieces, "stream max_group_pieces")?,
    );
    push_u64(
        &mut out,
        usize_to_u64(input.len(), "stream original input size")?,
    );
    push_u32(&mut out, usize_to_u32(total_chunks, "stream total_chunks")?);
    push_u32(&mut out, usize_to_u32(block_count, "stream block_count")?);
    if let Some(payload) = shared_grouping_payload_bytes.as_ref() {
        push_u32(
            &mut out,
            usize_to_u32(payload.len(), "stream shared grouping payload length")?,
        );
        out.extend_from_slice(payload);
    }

    let mut counts = StreamNodeCounts::default();
    let mut grouping_hint_updates = 0usize;
    let mut history_method: Option<PackMethod> = None;
    let enable_realtime_root_recursive_candidate = options.realtime_mode
        && options.max_group_depth >= 2
        && shared_grouping_payload_bytes.is_none();
    if options.realtime_mode
        && options.max_group_depth >= 2
        && shared_grouping_payload_bytes.is_some()
    {
        context.push_skipped(
            "local realtime recursive root candidates skipped: shared grouping payload is active"
        );
    }

    for block_index in 0..block_count {
        let block_start = block_index * options.key_piece_interval;
        let block_end = (block_start + options.key_piece_interval).min(total_chunks);
        let block_chunk_count = block_end - block_start;
        let block_original_len = chunks[block_start..block_end]
            .iter()
            .map(|chunk| chunk.len())
            .sum::<usize>();

        let planning_timer = Instant::now();
        let mut memo = HashMap::new();
        let local_root = build_best_stream_node(
            &chunks[block_start..block_end],
            0,
            block_chunk_count,
            block_start,
            options.max_group_depth,
            options.max_group_pieces,
            options.realtime_mode,
            enable_realtime_root_recursive_candidate,
            input_hash,
            &mut context,
            &mut memo,
        )?;
        context.timings.stream_block_planning_ms += planning_timer.elapsed().as_secs_f64() * 1000.0;
        let block_offset = block_start
            .checked_mul(options.chunk_size)
            .ok_or_else(|| ZbitError::Limit("stream block offset overflow".to_string()))?;
        let root = if shared_grouping_payload_bytes.is_some() {
            let global_slice =
                stream_global_slice_node(block_chunk_count, block_offset, block_original_len)?;
            if global_slice.encoded_size <= local_root.encoded_size {
                global_slice
            } else {
                local_root
            }
        } else {
            local_root
        };
        collect_stream_node_counts(&root, 0, &mut counts);

        let mut node_bytes = Vec::with_capacity(root.encoded_size);
        write_stream_node(&mut node_bytes, &root)?;
        if node_bytes.len() != root.encoded_size {
            return Err(ZbitError::Internal(format!(
                "stream node encoded size mismatch: expected {}, got {}",
                root.encoded_size,
                node_bytes.len()
            )));
        }

        push_u32(
            &mut out,
            usize_to_u32(block_start, "stream block first_chunk_index")?,
        );
        push_u32(
            &mut out,
            usize_to_u32(block_chunk_count, "stream block chunk_count")?,
        );
        push_u64(
            &mut out,
            usize_to_u64(block_original_len, "stream block original_len")?,
        );
        let history_field = if options.carry_grouping_history {
            history_method
                .map(|method| method.as_u8())
                .unwrap_or(ZBPS_HISTORY_NONE)
        } else {
            ZBPS_HISTORY_NONE
        };
        out.push(history_field);
        if history_field != ZBPS_HISTORY_NONE {
            grouping_hint_updates = grouping_hint_updates.saturating_add(1);
        }
        push_u32(
            &mut out,
            usize_to_u32(node_bytes.len(), "stream block node bytes")?,
        );
        out.extend_from_slice(&node_bytes);

        history_method = Some(stream_node_history_method(&root));
    }

    let stats = StreamPackStats {
        original_size: input.len(),
        compressed_size: out.len(),
        chunk_size: options.chunk_size,
        total_chunks,
        key_piece_interval: options.key_piece_interval,
        key_piece_count: block_count,
        block_count,
        max_group_depth: options.max_group_depth,
        max_group_pieces: options.max_group_pieces,
        piece_node_count: counts.piece_nodes,
        grouped_node_count: counts.grouped_nodes,
        split_node_count: counts.split_nodes,
        max_depth_used: counts.max_depth_used,
        grouping_hint_updates,
        key_piece_decode_note: format!(
            "receiver can start from chunk indices that are multiples of {} (block/key boundaries)",
            options.key_piece_interval
        ),
        effective_wide_overfitting_circuits: false,
        adaptive_wide_promotion_used: false,
        shared_grouping_payload_used: shared_grouping_payload_bytes.is_some(),
        active_profile: context.profile.name().to_string(),
        skipped_candidates: context.skipped_candidates,
        timings: context.timings,
        cache_stats: context.cache_stats,
    };

    Ok((out, stats))
}

pub fn compress_adaptive_stream_to_file(
    input: &[u8],
    path: impl AsRef<Path>,
    options: &StreamPackOptions,
) -> ZbitResult<StreamPackStats> {
    let (stream_bytes, stats) = compress_stream_to_bytes(input, options)?;
    fs::write(path.as_ref(), &stream_bytes)?;
    Ok(stats)
}

fn decompress_stream_from_bytes(
    bytes: &[u8],
    start_key_piece: Option<usize>,
) -> ZbitResult<Vec<u8>> {
    let mut cursor = 0usize;
    let header = parse_stream_header(bytes, &mut cursor)?;

    if cursor != ZBPS_HEADER_BYTES {
        return Err(ZbitError::Parse(format!(
            "stream header size mismatch: expected {ZBPS_HEADER_BYTES}, got {cursor}"
        )));
    }

    let has_global_grouping_payload = (header.flags & ZBPS_FLAG_WIDE_OVERFITTING_CIRCUITS) != 0
        || (header.flags & ZBPS_FLAG_SHARED_GROUPING_PAYLOAD) != 0;
    let global_overfit_output = if has_global_grouping_payload {
        let global_pack_len = read_u32(bytes, &mut cursor)? as usize;
        let global_pack_bytes = bytes.get(cursor..cursor + global_pack_len).ok_or_else(|| {
            ZbitError::Parse("stream global overfit payload range out of bounds".to_string())
        })?;
        cursor += global_pack_len;
        let decoded = decompress_pack_bytes(global_pack_bytes)?;
        if decoded.len() != header.original_size {
            return Err(ZbitError::Parse(format!(
                "stream global overfit output length mismatch: expected {} got {}",
                header.original_size,
                decoded.len()
            )));
        }
        Some(decoded)
    } else {
        None
    };

    if let Some(key_piece_index) = start_key_piece {
        if key_piece_index >= header.total_chunks {
            return Err(ZbitError::InvalidArg(
                "start_key_piece is outside stream chunk range",
            ));
        }
        if key_piece_index % header.key_piece_interval != 0 {
            return Err(ZbitError::InvalidArg(
                "start_key_piece must align with key_piece_interval boundary",
            ));
        }
    }

    let mut out = Vec::new();
    let mut total_decoded_chunks = 0usize;
    let mut total_decoded_bytes = 0usize;
    let mut started = start_key_piece.is_none();
    let start_key_piece_value = start_key_piece.unwrap_or(0);

    for expected_block_index in 0..header.block_count {
        let first_chunk_index = read_u32(bytes, &mut cursor)? as usize;
        let block_chunk_count = read_u32(bytes, &mut cursor)? as usize;
        let block_original_len = u64_to_usize(read_u64(bytes, &mut cursor)?, "stream block len")?;
        let _history_method_id = read_u8(bytes, &mut cursor)?;
        let node_bytes_len = read_u32(bytes, &mut cursor)? as usize;
        let node_bytes = bytes
            .get(cursor..cursor + node_bytes_len)
            .ok_or_else(|| ZbitError::Parse("stream block node range out of bounds".to_string()))?;
        cursor += node_bytes_len;

        let expected_first = expected_block_index
            .checked_mul(header.key_piece_interval)
            .ok_or_else(|| ZbitError::Parse("stream block index overflow".to_string()))?;
        if first_chunk_index != expected_first {
            return Err(ZbitError::Parse(format!(
                "stream block order mismatch: expected first chunk {expected_first}, got {first_chunk_index}"
            )));
        }

        if block_chunk_count == 0 {
            return Err(ZbitError::Parse(
                "stream block chunk_count must be > 0".to_string(),
            ));
        }
        let expected_block_len = expected_stream_block_len(
            header.original_size,
            header.chunk_size,
            header.total_chunks,
            first_chunk_index,
            block_chunk_count,
        )?;
        if expected_block_len != block_original_len {
            return Err(ZbitError::Parse(format!(
                "stream block length mismatch: header={} expected={expected_block_len}",
                block_original_len
            )));
        }

        if !started {
            if first_chunk_index == start_key_piece_value {
                started = true;
            } else {
                continue;
            }
        }

        let mut node_cursor = 0usize;
        let decoded = decode_stream_node(
            node_bytes,
            &mut node_cursor,
            global_overfit_output.as_deref(),
        )?;
        if node_cursor != node_bytes.len() {
            return Err(ZbitError::Parse(
                "trailing bytes in stream block node".to_string(),
            ));
        }
        if decoded.chunk_count != block_chunk_count {
            return Err(ZbitError::Parse(format!(
                "stream block chunk count mismatch: header={} decoded={}",
                block_chunk_count, decoded.chunk_count
            )));
        }
        if decoded.bytes.len() != block_original_len {
            return Err(ZbitError::Parse(format!(
                "stream block byte length mismatch: header={} decoded={}",
                block_original_len,
                decoded.bytes.len()
            )));
        }

        total_decoded_chunks = total_decoded_chunks
            .checked_add(decoded.chunk_count)
            .ok_or_else(|| ZbitError::Limit("stream decoded chunk count overflow".to_string()))?;
        total_decoded_bytes = total_decoded_bytes
            .checked_add(decoded.bytes.len())
            .ok_or_else(|| ZbitError::Limit("stream decoded size overflow".to_string()))?;
        out.extend_from_slice(&decoded.bytes);
    }

    if cursor != bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes after stream blocks".to_string(),
        ));
    }

    if start_key_piece.is_none() {
        if total_decoded_chunks != header.total_chunks {
            return Err(ZbitError::Parse(format!(
                "decoded stream chunks mismatch: expected {} got {}",
                header.total_chunks, total_decoded_chunks
            )));
        }
        if total_decoded_bytes != header.original_size {
            return Err(ZbitError::Parse(format!(
                "decoded stream length mismatch: expected {} got {}",
                header.original_size, total_decoded_bytes
            )));
        }
    } else if !started {
        return Err(ZbitError::Parse(
            "requested start_key_piece boundary not found in stream blocks".to_string(),
        ));
    }

    Ok(out)
}

pub fn decompress_stream_file(path: impl AsRef<Path>) -> ZbitResult<Vec<u8>> {
    let bytes = fs::read(path)?;
    decompress_stream_from_bytes(&bytes, None)
}

pub fn decompress_stream_file_from_key_piece(
    path: impl AsRef<Path>,
    key_piece_index: usize,
) -> ZbitResult<Vec<u8>> {
    let bytes = fs::read(path)?;
    decompress_stream_from_bytes(&bytes, Some(key_piece_index))
}

#[cfg(test)]
fn paeth_predictor(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn append_crc_frame(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        push_u32_be(out, data.len() as u32);
        out.extend_from_slice(chunk_type);
        out.extend_from_slice(data);
        let mut hasher = Crc32Hasher::new();
        hasher.update(chunk_type);
        hasher.update(data);
        push_u32_be(out, hasher.finalize());
    }

    fn build_framed_container_with_many_frames() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"ZBIT-FRAMED-PREFIX");

        let full_chunk_len = 128usize;
        let full_chunks = 96usize;
        let tail_chunk_len = 73usize;
        let total_len = full_chunk_len * full_chunks + tail_chunk_len;

        let mut payload = vec![0u8; total_len];
        let mut state = 0xA5A5_1337u32;
        for byte in &mut payload {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            *byte = (state >> 24) as u8;
        }

        let mut cursor = 0usize;
        for _ in 0..full_chunks {
            let slice = &payload[cursor..cursor + full_chunk_len];
            append_crc_frame(&mut out, b"DATA", slice);
            cursor += full_chunk_len;
        }

        append_crc_frame(&mut out, b"DATA", &payload[cursor..]);
        out.extend_from_slice(b"ZBIT-FRAMED-SUFFIX");
        out
    }

    fn build_valid_framed_container_with_split_deflate() -> (Vec<u8>, Vec<u8>) {
        let width = 64u32;
        let height = 64u32;

        let row_bytes = (width as usize) * 4;
        let mut filtered = Vec::with_capacity((row_bytes + 1) * (height as usize));
        let mut prev_raw = vec![0u8; row_bytes];

        for y in 0..height as usize {
            let filter = (y % 5) as u8;
            filtered.push(filter);

            let mut raw_row = vec![0u8; row_bytes];
            for x in 0..width as usize {
                let idx = x * 4;
                raw_row[idx] = ((x * 3 + y * 5) & 0xFF) as u8;
                raw_row[idx + 1] = ((x * 7 + y * 11) & 0xFF) as u8;
                raw_row[idx + 2] = ((x * 13 + y * 17) & 0xFF) as u8;
                raw_row[idx + 3] = 255u8;
            }

            for i in 0..row_bytes {
                let encoded = match filter {
                    0 => raw_row[i],
                    1 => {
                        let left = if i >= 4 { raw_row[i - 4] } else { 0 };
                        raw_row[i].wrapping_sub(left)
                    }
                    2 => raw_row[i].wrapping_sub(prev_raw[i]),
                    3 => {
                        let left = if i >= 4 { raw_row[i - 4] } else { 0 };
                        let up = prev_raw[i];
                        raw_row[i].wrapping_sub(((left as u16 + up as u16) / 2) as u8)
                    }
                    4 => {
                        let left = if i >= 4 { raw_row[i - 4] } else { 0 };
                        let up = prev_raw[i];
                        let up_left = if i >= 4 { prev_raw[i - 4] } else { 0 };
                        raw_row[i].wrapping_sub(paeth_predictor(left, up, up_left))
                    }
                    _ => unreachable!(),
                };
                filtered.push(encoded);
            }

            prev_raw.copy_from_slice(&raw_row);
        }

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&filtered).expect("zlib write");
        let framed_payload = encoder.finish().expect("zlib finish");

        let mut container = Vec::new();
        container.extend_from_slice(b"ZBIT-DEFLATE-PREFIX");

        let chunk = 1024usize;
        let mut cursor = 0usize;
        while cursor < framed_payload.len() {
            let end = (cursor + chunk).min(framed_payload.len());
            append_crc_frame(&mut container, b"DATA", &framed_payload[cursor..end]);
            cursor = end;
        }

        container.extend_from_slice(b"ZBIT-DEFLATE-SUFFIX");
        (container, filtered)
    }

    #[test]
    fn adaptive_pack_roundtrip() {
        let input = b"abcabcabcabc\nxyzxyzxyz\n";

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_test_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
    }

    #[test]
    fn adaptive_pack_can_choose_huffman_and_roundtrip() {
        let input = b"the quick brown fox jumps over the lazy dog\n".repeat(2000);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_huffman_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
        assert!(
            stats.indexed_huffman_candidate_bytes.is_some(),
            "huffman candidate should be evaluated for repetitive text"
        );
    }

    #[test]
    fn adaptive_pack_can_choose_raw_deflate_and_roundtrip() {
        let input = b"lorem ipsum dolor sit amet, consectetur adipiscing elit\\n".repeat(4000);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_deflate_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
        assert!(
            matches!(
                stats.chosen_method,
                PackMethod::RawDeflate | PackMethod::RawZstd | PackMethod::RawXz
            ),
            "expected a strong raw compressor, got {:?}",
            stats.chosen_method
        );
    }

    #[test]
    fn adaptive_pack_can_choose_raw_zstd_and_roundtrip() {
        let input =
            b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\nbbbbbbbbbbbbbbbbbbbbbbbb\\n".repeat(10_000);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_zstd_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
        assert!(stats.raw_zstd_candidate_bytes.is_some());
    }

    #[test]
    fn adaptive_pack_can_choose_monotonic_delta_and_roundtrip() {
        let mut input = Vec::new();
        let mut value = 10_000u64;
        let mut state = 0xC0FF_EE11u32;

        for _ in 0..90_000usize {
            write_le_u64_width(&mut input, value, 3).expect("write u24 value");
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let gap = ((state >> 27) as u64) + 1;
            value = value.saturating_add(gap);
        }

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_monotonic_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(
            matches!(stats.chosen_method, PackMethod::MonotonicDelta),
            "expected monotonic-delta to be chosen, got {:?}",
            stats.chosen_method
        );
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
        assert!(stats.monotonic_delta_candidate_bytes.is_some());
    }

    #[test]
    fn adaptive_pack_evaluates_framed_raw_and_roundtrips() {
        let input = build_framed_container_with_many_frames();

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_framed_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        let framed_candidate = stats
            .framed_raw_candidate_bytes
            .expect("framed-raw candidate should be available");
        assert!(
            framed_candidate < stats.raw_candidate_bytes,
            "framed-raw should beat raw-copy on multi-frame input"
        );
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
    }

    #[test]
    fn recursive_transform_roundtrip() {
        let (_container, filtered_plain) = build_valid_framed_container_with_split_deflate();
        let plan = CircuitTransformPlan {
            kind: CircuitTransformKind::PeriodicHeadTail,
            period: 257,
            head: 1,
        };
        let transformed = apply_transform_plan(&filtered_plain, &plan).expect("build transform");
        let decoded = invert_transform_plan(&transformed, filtered_plain.len(), &plan)
            .expect("decode transform");
        assert_eq!(decoded, filtered_plain);
    }

    #[test]
    fn adaptive_pack_evaluates_recursive_circuit_xz_candidate_and_roundtrips() {
        let (input, _filtered_plain) = build_valid_framed_container_with_split_deflate();

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_recursive_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        assert!(
            stats.recursive_circuit_xz_candidate_bytes.is_some(),
            "recursive-circuit-xz candidate should be available for valid framed deflate container"
        );
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
    }
}
