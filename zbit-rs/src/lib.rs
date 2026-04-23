// Licensed under the PolyForm Noncommercial License 1.0.0. See LICENSE.
// Copyright (c) 2026 Riccardo Cecchini <rcecchini.ds@gmail.com>.

pub mod error;
pub mod minimizer;
pub mod model;
pub mod pack;
pub mod pack_rules;

pub use error::{ZbitError, ZbitResult};
pub use model::{ZbitModel, ZbitStats, ZBIT_MAX_INPUTS_EXACT};
pub use pack::{compress_adaptive_to_file, decompress_file, PackStats};
pub use pack_rules::PackMethod;
