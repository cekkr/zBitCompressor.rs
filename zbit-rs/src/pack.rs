// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use crc32fast::Hasher as Crc32Hasher;
use crate::error::{ZbitError, ZbitResult};
use crate::model::ZbitModel;
use crate::pack_rules::{choose_best_method, should_evaluate_circuit, PackEvaluation, PackMethod};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use zstd::stream as zstd_stream;

pub const ZBPK_MAGIC: u32 = 0x5A42_504B; // "ZBPK"
pub const ZBPK_VERSION: u16 = 2;
pub const ZBPK_HEADER_BYTES: usize = 36;
const MAX_HUFFMAN_CODE_BITS: u8 = 56;
const ZBPK_MAX_OUTPUT_BYTES: usize = 1usize << 30; // 1 GiB hard safety bound

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
    pub png_idat_raw_candidate_bytes: Option<usize>,

    pub chosen_method: PackMethod,
    pub chosen_reason: String,
    pub circuit_rule_note: String,
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
struct PngIdatStream {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    payload: Vec<u8>,
    base_chunk_len: u32,
    full_chunk_count: u32,
    tail_chunk_len: u32,
    total_chunks: u32,
}

#[derive(Debug, Default)]
struct DecodeNode {
    symbol: Option<u8>,
    left: Option<Box<DecodeNode>>,
    right: Option<Box<DecodeNode>>,
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

fn decode_huffman_dictionary(dict_bytes: &[u8], unique_count: usize) -> ZbitResult<(Vec<u8>, Vec<u8>)> {
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
            let next = cursor.left.get_or_insert_with(|| Box::<DecodeNode>::default());
            cursor = next.as_mut();
        } else {
            let next = cursor.right.get_or_insert_with(|| Box::<DecodeNode>::default());
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

fn push_u32_be(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn read_u32_be_at(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn build_png_idat_stream(input: &[u8]) -> Option<PngIdatStream> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    const IDAT: &[u8; 4] = b"IDAT";
    const IEND: &[u8; 4] = b"IEND";

    if input.len() < 8 || &input[..8] != PNG_SIGNATURE {
        return None;
    }

    let mut cursor = 8usize;
    let mut first_idat_start: Option<usize> = None;
    let mut last_idat_end = 0usize;
    let mut saw_idat = false;
    let mut closed_idat_run = false;

    let mut chunk_lengths = Vec::<u32>::new();
    let mut payload = Vec::<u8>::new();

    while cursor.checked_add(12)? <= input.len() {
        let chunk_start = cursor;
        let chunk_len_u32 = read_u32_be_at(input, cursor)?;
        cursor += 4;

        let chunk_type = input.get(cursor..cursor + 4)?;
        cursor += 4;

        let chunk_len = chunk_len_u32 as usize;
        let data = input.get(cursor..cursor + chunk_len)?;
        cursor += chunk_len;

        let chunk_crc = read_u32_be_at(input, cursor)?;
        cursor += 4;

        if chunk_type == IDAT {
            if closed_idat_run {
                return None;
            }
            saw_idat = true;
            first_idat_start.get_or_insert(chunk_start);
            chunk_lengths.push(chunk_len_u32);
            payload.extend_from_slice(data);
            last_idat_end = cursor;

            let mut hasher = Crc32Hasher::new();
            hasher.update(IDAT);
            hasher.update(data);
            if hasher.finalize() != chunk_crc {
                return None;
            }
        } else if saw_idat {
            closed_idat_run = true;
        }

        if chunk_type == IEND {
            break;
        }
    }

    let first_start = first_idat_start?;
    if chunk_lengths.is_empty() || last_idat_end <= first_start {
        return None;
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
            return None;
        }

        full_chunk_count = total_chunks.saturating_sub(1);
        tail_chunk_len = *chunk_lengths.last().unwrap_or(&0u32);
    }

    let prefix = input[..first_start].to_vec();
    let suffix = input[last_idat_end..].to_vec();

    Some(PngIdatStream {
        prefix,
        suffix,
        payload,
        base_chunk_len,
        full_chunk_count,
        tail_chunk_len,
        total_chunks,
    })
}

fn png_idat_dictionary_size(stream: &PngIdatStream) -> usize {
    24usize + stream.prefix.len() + stream.suffix.len()
}

fn write_png_idat_dictionary(out: &mut Vec<u8>, stream: &PngIdatStream) {
    push_u32(out, stream.prefix.len() as u32);
    push_u32(out, stream.suffix.len() as u32);
    push_u32(out, stream.base_chunk_len);
    push_u32(out, stream.full_chunk_count);
    push_u32(out, stream.tail_chunk_len);
    push_u32(out, stream.total_chunks);
    out.extend_from_slice(&stream.prefix);
    out.extend_from_slice(&stream.suffix);
}

fn decode_png_idat_payload(dict_bytes: &[u8], payload: &[u8], original_size: usize) -> ZbitResult<Vec<u8>> {
    let mut dict_cursor = 0usize;
    let prefix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let suffix_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let base_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let full_chunk_count = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let tail_chunk_len = read_u32(dict_bytes, &mut dict_cursor)? as usize;
    let total_chunks = read_u32(dict_bytes, &mut dict_cursor)? as usize;

    let prefix = dict_bytes
        .get(dict_cursor..dict_cursor + prefix_len)
        .ok_or_else(|| ZbitError::Parse("png-idat-raw prefix range out of bounds".to_string()))?;
    dict_cursor += prefix_len;

    let suffix = dict_bytes
        .get(dict_cursor..dict_cursor + suffix_len)
        .ok_or_else(|| ZbitError::Parse("png-idat-raw suffix range out of bounds".to_string()))?;
    dict_cursor += suffix_len;

    if dict_cursor != dict_bytes.len() {
        return Err(ZbitError::Parse(
            "trailing bytes in png-idat-raw dictionary".to_string(),
        ));
    }

    let tail_present = if total_chunks == full_chunk_count {
        false
    } else if total_chunks == full_chunk_count + 1 {
        true
    } else {
        return Err(ZbitError::Parse(
            "png-idat-raw dictionary has inconsistent chunk counters".to_string(),
        ));
    };

    let expected_payload = full_chunk_count
        .checked_mul(base_chunk_len)
        .and_then(|v| if tail_present { v.checked_add(tail_chunk_len) } else { Some(v) })
        .ok_or_else(|| ZbitError::Parse("png-idat-raw payload length overflow".to_string()))?;

    if payload.len() != expected_payload {
        return Err(ZbitError::Parse(format!(
            "png-idat-raw payload length mismatch: expected {expected_payload} got {}",
            payload.len()
        )));
    }

    let chunk_overhead = total_chunks
        .checked_mul(12)
        .ok_or_else(|| ZbitError::Parse("png-idat-raw chunk overhead overflow".to_string()))?;
    let mut out = Vec::with_capacity(
        prefix
            .len()
            .checked_add(payload.len())
            .and_then(|v| v.checked_add(suffix.len()))
            .and_then(|v| v.checked_add(chunk_overhead))
            .ok_or_else(|| ZbitError::Parse("png-idat-raw output length overflow".to_string()))?,
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
            .ok_or_else(|| ZbitError::Parse("png-idat-raw payload chunk range out of bounds".to_string()))?;
        payload_cursor += chunk_len;

        push_u32_be(&mut out, chunk_len as u32);
        out.extend_from_slice(b"IDAT");
        out.extend_from_slice(chunk_data);

        let mut hasher = Crc32Hasher::new();
        hasher.update(b"IDAT");
        hasher.update(chunk_data);
        push_u32_be(&mut out, hasher.finalize());
    }

    out.extend_from_slice(suffix);

    if out.len() != original_size {
        return Err(ZbitError::Parse(format!(
            "png-idat-raw output length mismatch: expected {original_size} got {}",
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
    png_idat_stream: Option<&PngIdatStream>,
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
                ZbitError::Internal("raw-deflate payload missing for raw-deflate method".to_string())
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::RawZstd => {
            let payload = raw_zstd_payload.ok_or_else(|| {
                ZbitError::Internal("raw-zstd payload missing for raw-zstd method".to_string())
            })?;
            (0u8, 0usize, 0usize, payload.len())
        }
        PackMethod::PngIdatRaw => {
            let png = png_idat_stream.ok_or_else(|| {
                ZbitError::Internal("png-idat stream missing for png-idat-raw method".to_string())
            })?;
            (0u8, 0usize, png_idat_dictionary_size(png), png.payload.len())
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
                ZbitError::Internal("huffman stream missing for indexed-huffman dictionary".to_string())
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
        PackMethod::RawDeflate | PackMethod::RawZstd => {}
        PackMethod::PngIdatRaw => {
            let png = png_idat_stream.ok_or_else(|| {
                ZbitError::Internal("png-idat stream missing for png-idat-raw dictionary".to_string())
            })?;
            write_png_idat_dictionary(&mut out, png);
        }
    }

    match method {
        PackMethod::RawCopy => out.extend_from_slice(input),
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => out.extend_from_slice(&stream.payload),
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.ok_or_else(|| {
                ZbitError::Internal("huffman stream missing for indexed-huffman payload".to_string())
            })?;
            out.extend_from_slice(&hs.payload);
        }
        PackMethod::RawDeflate => {
            let payload = raw_deflate_payload.ok_or_else(|| {
                ZbitError::Internal("raw-deflate payload missing for raw-deflate method".to_string())
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::RawZstd => {
            let payload = raw_zstd_payload.ok_or_else(|| {
                ZbitError::Internal("raw-zstd payload missing for raw-zstd method".to_string())
            })?;
            out.extend_from_slice(&payload);
        }
        PackMethod::PngIdatRaw => {
            let png = png_idat_stream.ok_or_else(|| {
                ZbitError::Internal("png-idat stream missing for png-idat-raw payload".to_string())
            })?;
            out.extend_from_slice(&png.payload);
        }
    }

    Ok(out)
}

pub fn compress_adaptive_to_file(input: &[u8], path: impl AsRef<Path>) -> ZbitResult<PackStats> {
    let stream = build_index_stream(input)?;

    let raw_candidate_bytes = ZBPK_HEADER_BYTES + input.len();
    let indexed_raw_candidate_bytes = ZBPK_HEADER_BYTES + stream.unique_symbols.len() + stream.payload.len();

    let huffman_stream = build_huffman_stream(input, &stream)?;
    let indexed_huffman_candidate_bytes = huffman_stream
        .as_ref()
        .map(|hs| ZBPK_HEADER_BYTES + hs.symbols.len() * 2 + hs.payload.len());
    let raw_deflate_payload = build_raw_deflate_payload(input)?;
    let raw_deflate_candidate_bytes = Some(ZBPK_HEADER_BYTES + raw_deflate_payload.len());
    let raw_zstd_payload = build_raw_zstd_payload(input)?;
    let raw_zstd_candidate_bytes = Some(ZBPK_HEADER_BYTES + raw_zstd_payload.len());
    let png_idat_stream = build_png_idat_stream(input);
    let png_idat_raw_candidate_bytes = png_idat_stream
        .as_ref()
        .map(|png| ZBPK_HEADER_BYTES + png_idat_dictionary_size(png) + png.payload.len());

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
    eval.png_idat_raw_total_bytes = png_idat_raw_candidate_bytes;

    let (should_eval_circuit, circuit_rule_note) = should_evaluate_circuit(&eval);

    let (circuit_blobs, circuit_dictionary_bytes, indexed_circuit_candidate_bytes) = if should_eval_circuit {
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
        png_idat_stream.as_ref(),
    )?;

    fs::write(path.as_ref(), &pack_bytes)?;

    let (bits_per_symbol, payload_bytes, huffman_dictionary_bytes) = match eval.chosen_method {
        PackMethod::RawCopy => (0u8, input.len(), 0usize),
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => (stream.bits_per_symbol, stream.payload.len(), 0usize),
        PackMethod::IndexedHuffman => {
            let hs = huffman_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal("indexed-huffman selected without huffman stream".to_string())
            })?;
            (0u8, hs.payload.len(), hs.symbols.len() * 2)
        }
        PackMethod::RawDeflate => (0u8, raw_deflate_payload.len(), 0usize),
        PackMethod::RawZstd => (0u8, raw_zstd_payload.len(), 0usize),
        PackMethod::PngIdatRaw => {
            let png = png_idat_stream.as_ref().ok_or_else(|| {
                ZbitError::Internal("png-idat-raw selected without png stream".to_string())
            })?;
            (0u8, png.payload.len(), 0usize)
        }
    };

    Ok(PackStats {
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
        png_idat_raw_candidate_bytes,
        chosen_method: eval.chosen_method,
        chosen_reason: eval.chosen_reason,
        circuit_rule_note,
    })
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

pub fn decompress_file(path: impl AsRef<Path>) -> ZbitResult<Vec<u8>> {
    let bytes = fs::read(path)?;
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
        return Err(ZbitError::Parse("non-zero flags are unsupported".to_string()));
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
            if bits_per_symbol != 0 || unique_count != 0 || dict_size != 0 || payload_size != original_size {
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
                    "raw-deflate requires bits_per_symbol=0, unique_count=0, dict_size=0".to_string(),
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
        PackMethod::PngIdatRaw => {
            if bits_per_symbol != 0 || unique_count != 0 {
                return Err(ZbitError::Parse(
                    "png-idat-raw requires bits_per_symbol=0 and unique_count=0".to_string(),
                ));
            }
            return decode_png_idat_payload(dict, payload, original_size);
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
        | PackMethod::PngIdatRaw => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn append_png_chunk(out: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        push_u32_be(out, data.len() as u32);
        out.extend_from_slice(chunk_type);
        out.extend_from_slice(data);
        let mut hasher = Crc32Hasher::new();
        hasher.update(chunk_type);
        hasher.update(data);
        push_u32_be(out, hasher.finalize());
    }

    fn build_png_with_many_idat_chunks() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&1u32.to_be_bytes()); // width
        ihdr.extend_from_slice(&1u32.to_be_bytes()); // height
        ihdr.push(8); // bit depth
        ihdr.push(6); // color type RGBA
        ihdr.push(0); // compression
        ihdr.push(0); // filter
        ihdr.push(0); // interlace
        append_png_chunk(&mut out, b"IHDR", &ihdr);

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
            append_png_chunk(&mut out, b"IDAT", slice);
            cursor += full_chunk_len;
        }

        append_png_chunk(&mut out, b"IDAT", &payload[cursor..]);
        append_png_chunk(&mut out, b"IEND", &[]);
        out
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
            matches!(stats.chosen_method, PackMethod::RawDeflate | PackMethod::RawZstd),
            "expected a strong raw compressor, got {:?}",
            stats.chosen_method
        );
    }

    #[test]
    fn adaptive_pack_can_choose_raw_zstd_and_roundtrip() {
        let input = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\\nbbbbbbbbbbbbbbbbbbbbbbbb\\n".repeat(10_000);

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
    fn adaptive_pack_evaluates_png_idat_raw_and_roundtrips() {
        let input = build_png_with_many_idat_chunks();

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zbit_pack_png_idat_{stamp}.zbpk"));

        let stats = compress_adaptive_to_file(&input, &path).expect("compress adaptive");
        let output = decompress_file(&path).expect("decompress adaptive");
        let _ = fs::remove_file(&path);

        assert_eq!(output, input);
        let png_candidate = stats
            .png_idat_raw_candidate_bytes
            .expect("png-idat-raw candidate should be available");
        assert!(
            png_candidate < stats.raw_candidate_bytes,
            "png-idat-raw should beat raw-copy on multi-IDAT input"
        );
        assert!(stats.compressed_size <= stats.raw_candidate_bytes);
    }
}
