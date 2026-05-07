// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

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

