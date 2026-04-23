use std::fs;
use std::path::Path;

use crate::error::{ZbitError, ZbitResult};
use crate::model::ZbitModel;
use crate::pack_rules::{choose_best_method, should_evaluate_circuit, PackEvaluation, PackMethod};

pub const ZBPK_MAGIC: u32 = 0x5A42_504B; // "ZBPK"
pub const ZBPK_VERSION: u16 = 2;
pub const ZBPK_HEADER_BYTES: usize = 36;

#[derive(Debug, Clone)]
pub struct PackStats {
    pub original_size: usize,
    pub compressed_size: usize,

    pub unique_symbols: usize,
    pub bits_per_symbol: u8,
    pub payload_bytes: usize,

    pub raw_dictionary_bytes: usize,
    pub circuit_dictionary_bytes: usize,

    pub raw_candidate_bytes: usize,
    pub indexed_raw_candidate_bytes: usize,
    pub indexed_circuit_candidate_bytes: Option<usize>,

    pub chosen_method: PackMethod,
    pub chosen_reason: String,
    pub circuit_rule_note: String,
}

#[derive(Debug, Clone)]
struct IndexStream {
    unique_symbols: Vec<u8>,
    bits_per_symbol: u8,
    payload: Vec<u8>,
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

fn build_index_stream(input: &[u8]) -> ZbitResult<IndexStream> {
    let mut present = [false; 256];
    let mut id_map = [u16::MAX; 256];

    for &b in input {
        present[b as usize] = true;
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
    })
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

fn write_pack_bytes(
    method: PackMethod,
    input: &[u8],
    stream: &IndexStream,
    circuit_blobs: Option<&[Vec<u8>]>,
    circuit_dict_bytes: usize,
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
    }

    match method {
        PackMethod::RawCopy => out.extend_from_slice(input),
        PackMethod::IndexedRaw | PackMethod::IndexedCircuit => out.extend_from_slice(&stream.payload),
    }

    Ok(out)
}

pub fn compress_adaptive_to_file(input: &[u8], path: impl AsRef<Path>) -> ZbitResult<PackStats> {
    let stream = build_index_stream(input)?;

    let raw_candidate_bytes = ZBPK_HEADER_BYTES + input.len();
    let indexed_raw_candidate_bytes = ZBPK_HEADER_BYTES + stream.unique_symbols.len() + stream.payload.len();

    let mut eval = PackEvaluation::new();
    eval.original_size = input.len();
    eval.symbol_bits = 8;
    eval.unique_symbols = stream.unique_symbols.len();
    eval.payload_bytes = stream.payload.len();
    eval.raw_total_bytes = raw_candidate_bytes;
    eval.indexed_raw_total_bytes = indexed_raw_candidate_bytes;

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
    )?;

    fs::write(path.as_ref(), &pack_bytes)?;

    Ok(PackStats {
        original_size: input.len(),
        compressed_size: pack_bytes.len(),
        unique_symbols: stream.unique_symbols.len(),
        bits_per_symbol: stream.bits_per_symbol,
        payload_bytes: stream.payload.len(),
        raw_dictionary_bytes: stream.unique_symbols.len(),
        circuit_dictionary_bytes,
        raw_candidate_bytes,
        indexed_raw_candidate_bytes,
        indexed_circuit_candidate_bytes,
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
        PackMethod::RawCopy => unreachable!(),
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
}
