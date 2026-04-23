// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PackMethod {
    RawCopy,
    IndexedRaw,
    IndexedCircuit,
}

impl PackMethod {
    pub fn as_u8(self) -> u8 {
        match self {
            Self::RawCopy => 0,
            Self::IndexedRaw => 1,
            Self::IndexedCircuit => 2,
        }
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::RawCopy),
            1 => Some(Self::IndexedRaw),
            2 => Some(Self::IndexedCircuit),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::RawCopy => "raw-copy",
            Self::IndexedRaw => "indexed-raw",
            Self::IndexedCircuit => "indexed-circuit",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PackEvaluation {
    pub original_size: usize,
    pub symbol_bits: usize,
    pub unique_symbols: usize,
    pub payload_bytes: usize,

    pub raw_total_bytes: usize,
    pub indexed_raw_total_bytes: usize,

    pub indexed_circuit_total_bytes: Option<usize>,

    pub chosen_method: PackMethod,
    pub chosen_reason: String,
}

impl PackEvaluation {
    pub fn new() -> Self {
        Self {
            original_size: 0,
            symbol_bits: 0,
            unique_symbols: 0,
            payload_bytes: 0,
            raw_total_bytes: 0,
            indexed_raw_total_bytes: 0,
            indexed_circuit_total_bytes: None,
            chosen_method: PackMethod::RawCopy,
            chosen_reason: String::new(),
        }
    }
}

pub fn should_evaluate_circuit(eval: &PackEvaluation) -> (bool, String) {
    if eval.unique_symbols == 0 {
        return (false, "empty input: no circuit dictionary needed".to_string());
    }

    if eval.symbol_bits <= 8 {
        return (
            false,
            "symbol width <= 8 bits: raw dictionary is denser than circuit descriptors".to_string(),
        );
    }

    if eval.unique_symbols > 64 {
        return (
            false,
            "too many unique symbols: circuit dictionary overhead likely dominates".to_string(),
        );
    }

    if eval.payload_bytes < 4096 {
        return (
            false,
            "payload too small: circuit dictionary overhead cannot be amortized".to_string(),
        );
    }

    (true, "circuit candidate allowed by heuristic rules".to_string())
}

pub fn choose_best_method(eval: &mut PackEvaluation) {
    let mut best_method = PackMethod::RawCopy;
    let mut best_size = eval.raw_total_bytes;
    let mut best_reason = format!("raw-copy baseline selected ({} bytes)", eval.raw_total_bytes);

    if eval.indexed_raw_total_bytes < best_size {
        best_method = PackMethod::IndexedRaw;
        best_size = eval.indexed_raw_total_bytes;
        best_reason = format!(
            "indexed-raw improves size: {} -> {} bytes",
            eval.raw_total_bytes, eval.indexed_raw_total_bytes
        );
    }

    if let Some(circuit_size) = eval.indexed_circuit_total_bytes {
        let threshold = best_size.saturating_sub(best_size / 100);
        if circuit_size < threshold {
            best_method = PackMethod::IndexedCircuit;
            best_size = circuit_size;
            best_reason = format!("indexed-circuit chosen after rules: {circuit_size} bytes");
        }
    }

    eval.chosen_method = best_method;
    eval.chosen_reason = best_reason;
    let _ = best_size;
}
