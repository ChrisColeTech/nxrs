//! `nxrs-build shader-xlate <shaders folder>`: the live translator for a web build's server (docs/waves/WEB-W7b.md).
//! One shader the game made at run time arrives on stdin (`shaders::wire::request`: the ctl and code blobs the
//! native live path hands `translate_one`); it is translated exactly as a native run translates it, into
//! `<shaders folder>\live\<key>\`, and its key is printed. The next build promotes it into the library.

use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::Path;

/// Translates the request in `body` into `<shaders>/live`; its key.
pub fn run(shaders: &Path, body: &[u8]) -> Result<String> {
    if !shaders.is_dir() {
        return Err(anyhow!("{} is not a shader folder", shaders.display()));
    }
    let (ctl, code) = shaders::wire::parse_request(body)
        .ok_or_else(|| anyhow!("the request is not a shader (u32 ctl length, ctl, code; at most {} bytes)", shaders::wire::MAX_REQUEST))?;
    shaders::prebuild::translate_one(ctl, code, &shaders.join("live")).map_err(|e| anyhow!(e))
}

/// The command: the request from stdin, the key on stdout.
pub fn command(shaders: &Path) -> Result<String> {
    let mut body = Vec::new();
    std::io::stdin()
        .take(shaders::wire::MAX_REQUEST as u64 + 1)
        .read_to_end(&mut body)?;
    run(shaders, &body)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_request_that_is_not_a_shader_is_refused_before_translating() {
        let dir = std::env::temp_dir().join(format!("nxrs-shaderxlate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let e = super::run(&dir, &[1, 2, 3]).unwrap_err().to_string();
        assert!(e.contains("not a shader"), "{e}");
        let e = super::run(&dir.join("nope"), &shaders::wire::request(b"c", b"d")).unwrap_err();
        assert!(e.to_string().contains("not a shader folder"), "{e}");
        assert!(!dir.join("live").exists(), "nothing written");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
