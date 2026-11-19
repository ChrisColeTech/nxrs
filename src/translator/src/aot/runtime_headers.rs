//! The headers the generated C includes (`cpu.h`, `simd.h` and `simd_fp.h`). They are written next to the
//! generated C so a chunk compiles on its own with `-I <out>`.

use std::path::Path;

pub const HEADERS: [(&str, &str); 3] = [
    ("cpu.h", include_str!("../../runtime/cpu.h")),
    ("simd.h", include_str!("../../runtime/simd.h")),
    ("simd_fp.h", include_str!("../../runtime/simd_fp.h")),
];

/// Writes the headers into `out/include/`; returns how many there are.
pub fn write(out: &Path) -> std::io::Result<usize> {
    let dir = out.join("include");
    std::fs::create_dir_all(&dir)?;
    for (name, text) in HEADERS {
        crate::aot::gen::write_if_changed(&dir.join(name), text.as_bytes())?;
    }
    Ok(HEADERS.len())
}
