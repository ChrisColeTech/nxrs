//! The engine's media devices and codecs: the sound card (`sink`), the capture device (`mic`), libopus (`opus`), libvpx's VP9 (`vpx`), dav1d's AV1 (`av1`), Media Foundation's H.264 and AAC (`mf`),
//! the pictures they decode (`yuv`),
//! and the C runtime pieces the codec archives expect (`crt`).

pub mod av1;
// the MinGW runtime pieces exist under their own names in a gnullvm (MinGW) link
#[cfg(target_env = "msvc")]
pub mod crt;
pub mod mf;
pub mod mic;
pub mod opus;
pub mod sink;
pub mod vpx;
pub mod yuv;
