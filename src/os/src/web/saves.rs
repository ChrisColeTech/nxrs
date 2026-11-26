//! Saves in the browser (docs/35 W3): a committed save folder is packed (`webdata::pack_save`) and stored by the I/O
//! worker in the origin-private file system (`nxrs-saves/<title>/<save>.bin`, written as `.new`, flushed, then moved
//! over the old one). The slot's done state is the acknowledgement: a commit the worker did not confirm within 10 s
//! fails with a red line. The page restores every stored save into the in-memory file system before `main` starts
//! (`src/web/saves.js`), and refuses to start when it cannot read them.

use std::path::Path;
use std::time::Instant;

const ACK_MS: f64 = 10_000.0;

/// Stores the committed save folder `committed` (under the save root `root`) in the browser.
pub fn persist(root: &Path, committed: &Path) -> Result<(), String> {
    let name = committed
        .strip_prefix(root)
        .map_err(|_| format!("{} is not under {}", committed.display(), root.display()))?
        .to_string_lossy()
        .replace('\\', "/");
    let blob = crate::webdata::pack_save(committed).map_err(|e| format!("packing it: {e}"))?;
    let t0 = Instant::now();
    let r = super::io::request(
        super::io::OP_SAVE,
        name.len() as u32,
        name.as_ptr() as u64,
        blob.as_ptr() as u64,
        blob.len() as u32,
        Some(ACK_MS),
    );
    match r {
        Some(0) => {
            crate::log!(
                "web: save {name} stored in the browser ({} bytes, acknowledged in {:.0} ms)",
                blob.len(),
                t0.elapsed().as_secs_f64() * 1e3
            );
            Ok(())
        }
        Some(e) => Err(format!("the I/O worker could not store it (error {e})")),
        None => {
            // the worker may still read them: they stay alive
            std::mem::forget(blob);
            std::mem::forget(name);
            Err(format!("not acknowledged within {} s", ACK_MS / 1e3))
        }
    }
}
