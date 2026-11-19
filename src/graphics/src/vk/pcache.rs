//! The pipeline cache on disk, saved incrementally: a cache written only at exit is never written when the process
//! is killed. A saver thread writes it a few seconds after new pipelines stop arriving (`NXGFX_PCACHE_QUIET_MS`,
//! 2000), through a temporary file and an atomic replace, so a crash or a kill at any moment leaves either the
//! previous file or the new one. A file is used only when its header names this device and driver
//! (`pipelineCacheUUID`); it is merged into the live cache when the game's data folder is known.

use super::Gpu;
use ash::vk;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

const FILE: &str = "pipeline-cache.bin";

static CREATED: AtomicU64 = AtomicU64::new(0);
static SAVED_AT: AtomicU64 = AtomicU64::new(0);
static LAST_NS: AtomicU64 = AtomicU64::new(0);
static SAVES: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);
static SAVE_NS: AtomicU64 = AtomicU64::new(0);
static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);
static LOCK: Mutex<()> = Mutex::new(());

/// Whether `data` is a pipeline cache this device and driver made.
pub fn header_ok(data: &[u8], props: &vk::PhysicalDeviceProperties) -> bool {
    if data.len() < 32 {
        return false;
    }
    let u = |o: usize| u32::from_le_bytes(data[o..o + 4].try_into().unwrap());
    u(4) == vk::PipelineCacheHeaderVersion::ONE.as_raw() as u32 && u(8) == props.vendor_id && u(12) == props.device_id && data[16..32] == props.pipeline_cache_uuid
}

/// The file in `dir` merged into the live cache, and the saver started for it.
pub fn attach(g: &'static Gpu, dir: &Path) {
    let path = dir.join(FILE);
    let first = PATH.lock().unwrap().replace(path.clone()).is_none();
    let data = std::fs::read(&path).unwrap_or_default();
    let mut loaded = 0;
    if !data.is_empty() {
        if header_ok(&data, &g.d.props) {
            let ci = vk::PipelineCacheCreateInfo::default().initial_data(&data);
            if let Ok(c) = unsafe { g.dev().create_pipeline_cache(&ci, None) } {
                if unsafe { g.dev().merge_pipeline_caches(g.pipes.cache, &[c]) }.is_ok() {
                    loaded = data.len();
                }
                unsafe { g.dev().destroy_pipeline_cache(c, None) };
            }
        } else {
            crate::vklog!("pipeline cache file {} is from another device or driver: started empty", path.display());
        }
    }
    let quiet = quiet_ms();
    crate::vklog!("pipeline cache {}: {loaded} bytes loaded; saved incrementally ({quiet} ms after the last new pipeline)", path.display());
    if first {
        std::thread::Builder::new()
            .name("vk pipeline cache".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                let c = CREATED.load(Ordering::Relaxed);
                if c == SAVED_AT.load(Ordering::Relaxed) {
                    continue;
                }
                if crate::nvn::device::now_ns().saturating_sub(LAST_NS.load(Ordering::Relaxed)) < quiet * 1_000_000 {
                    continue;
                }
                save_now(g);
            })
            .ok();
    }
}

fn quiet_ms() -> u64 {
    std::env::var("NXGFX_PCACHE_QUIET_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(2000)
}

/// `n` new pipelines went into the cache.
pub fn note_created(n: u64) {
    CREATED.fetch_add(n, Ordering::Relaxed);
    LAST_NS.store(crate::nvn::device::now_ns(), Ordering::Relaxed);
}

/// Writes the cache now (temporary file, then an atomic replace). True on success.
pub fn save_now(g: &Gpu) -> bool {
    let Some(path) = PATH.lock().unwrap().clone() else { return false };
    let t0 = std::time::Instant::now();
    let _l = LOCK.lock().unwrap();
    let c = CREATED.load(Ordering::Relaxed);
    let Ok(data) = (unsafe { g.dev().get_pipeline_cache_data(g.pipes.cache) }) else { return false };
    let tmp = path.with_extension("bin.tmp");
    let ok = (|| -> std::io::Result<()> {
        use std::io::Write;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&data)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &path)
    })()
    .is_ok();
    if ok {
        SAVED_AT.store(c, Ordering::Relaxed);
        SAVES.fetch_add(1, Ordering::Relaxed);
        BYTES.store(data.len() as u64, Ordering::Relaxed);
    } else {
        crate::log::red_once(&format!("pipeline cache {} could not be written", path.display()));
    }
    SAVE_NS.fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    ok
}

/// (pipelines created, saves, bytes of the last save, save time ns).
pub fn stats() -> (u64, u64, u64, u64) {
    (CREATED.load(Ordering::Relaxed), SAVES.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed), SAVE_NS.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    #[test]
    fn header_check() {
        let mut p = ash::vk::PhysicalDeviceProperties { vendor_id: 0x10de, device_id: 0x2b85, ..Default::default() };
        p.pipeline_cache_uuid = [7; 16];
        let mut d = vec![0u8; 64];
        d[0..4].copy_from_slice(&32u32.to_le_bytes());
        d[4..8].copy_from_slice(&1u32.to_le_bytes());
        d[8..12].copy_from_slice(&0x10deu32.to_le_bytes());
        d[12..16].copy_from_slice(&0x2b85u32.to_le_bytes());
        d[16..32].copy_from_slice(&[7; 16]);
        assert!(super::header_ok(&d, &p));
        d[20] = 0;
        assert!(!super::header_ok(&d, &p));
    }
}
