//! ZBIC (Switch 2 NSO segments): the engine's one decoder lives in the translator's loader.

pub use translator::loader::zbic::{decompress, is_zbic, to_zstd, MAGIC};
