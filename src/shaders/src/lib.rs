//! The RecompLauncher engine's shaders: every shader of a game found when the game is built, translated to SPIR-V
//! for Vulkan and validated, before the game ever runs.
//!
//! | Module | What |
//! |---|---|
//! | [`nvn`] | control section + code area, the runtime's shader key, stages, checks |
//! | [`formats`] | the readers: BNSH, BAHS, NDSR / NDVE / sead, SHN!, MSHD, GLSLC outputs; Yaz0, SARC, zstd (+ dictionaries), gzip, size-prefixed frames, Oodle; GFPAK, Trinity packs, CRI CPK, Snowdrop, Heaps, PACx 403 + Needle; ZBIC / LZ4 NSO segments |
//! | [`scan`] | a whole game: ExeFS code modules + RomFS (update applied), in parallel, one copy per key, vertex / pixel pairs |
//! | [`job`] | a shader → the translator's job (Maxwell and Ampere code, c1, compute and mesh facts) |
//! | [`xlate`] | the translator (`dmsh_xlate`, NativeAOT) driven in parallel, and `spirv-val` |
//! | [`mesh`] | NVN2 task and mesh shaders made Vulkan task and mesh stages around the translated kernel |
//! | [`fixes`] | the SPIR-V edits Vulkan needs (capabilities the translator leaves out, out-of-range size-query components) |
//! | [`reflect`] | the SPIR-V's descriptor bindings, as the graphics module creates its layouts |
//! | [`prebuild`] | the whole build step: scan, translate, validate, write the output and its manifest |
//!
//! Credits and licenses: the repository's README.md.

pub mod fixes;
pub mod formats;
pub mod job;
pub mod mesh;
pub mod rt;
pub mod nvn;
pub mod prebuild;
pub mod reflect;
pub mod scan;
pub mod xlate;

pub use prebuild::{prebuild, prebuild_data, prebuild_game, Options, Report};
