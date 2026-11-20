//! Covers and backgrounds as textures: decoded on two worker threads, uploaded a few a frame, keyed by
//! path + size + modification time, the cache bounded in bytes (oldest first out).

use crate::constants::layout;
use crate::flows::{decode_artwork, file_stamp};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

type Key = (String, u64, u128);
type Decoded = (Key, Result<image::RgbaImage, String>);

#[derive(Clone)]
pub enum Art {
    Loading,
    Ready(egui::TextureHandle),
    Failed(String),
}

pub struct ArtworkCache {
    entries: HashMap<Key, Art>,
    order: VecDeque<Key>,
    bytes: usize,
    requests: Sender<Key>,
    results: Receiver<Decoded>,
    pub epoch: u64,
}

impl ArtworkCache {
    pub fn new(notify: Arc<dyn Fn() + Send + Sync>) -> ArtworkCache {
        let (req_tx, req_rx) = channel::<Key>();
        let (res_tx, res_rx) = channel::<Decoded>();
        let req_rx = Arc::new(Mutex::new(req_rx));
        for _ in 0..layout::ARTWORK_WORKERS {
            let (rx, tx, notify) = (req_rx.clone(), res_tx.clone(), notify.clone());
            std::thread::spawn(move || {
                while let Some(key) = rx.lock().ok().and_then(|r| r.recv().ok()) {
                    let result = decode_artwork(&key.0);
                    if tx.send((key, result)).is_err() {
                        return;
                    }
                    notify();
                }
            });
        }
        ArtworkCache {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            requests: req_tx,
            results: res_rx,
            epoch: 0,
        }
    }

    /// The image for a path: asked for the first time, `Loading` until it arrives. None for no path.
    pub fn get(&mut self, path: &str) -> Option<Art> {
        if path.is_empty() {
            return None;
        }
        let Some((size, modified)) = file_stamp(path) else {
            return Some(Art::Failed("The file does not exist".into()));
        };
        let key = (path.to_string(), size, modified);
        if let Some(a) = self.entries.get(&key) {
            return Some(a.clone());
        }
        self.entries.insert(key.clone(), Art::Loading);
        let _ = self.requests.send(key);
        Some(Art::Loading)
    }

    /// Uploads what the workers finished, a few a frame; drops the oldest over the budget.
    pub fn upload(&mut self, ctx: &egui::Context) {
        for (key, result) in self
            .results
            .try_iter()
            .take(layout::ARTWORK_UPLOADS)
            .collect::<Vec<_>>()
        {
            if !self.entries.contains_key(&key) {
                continue; // invalidated meanwhile
            }
            let art = match result {
                Err(e) => Art::Failed(e),
                Ok(img) => {
                    let size = [img.width() as usize, img.height() as usize];
                    self.bytes += size[0] * size[1] * 4;
                    let color = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                    self.order.push_back(key.clone());
                    Art::Ready(ctx.load_texture(
                        format!("art:{}", key.0),
                        color,
                        egui::TextureOptions::LINEAR,
                    ))
                }
            };
            self.entries.insert(key, art);
            ctx.request_repaint();
        }
        while self.bytes > layout::ARTWORK_BUDGET {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some(Art::Ready(t)) = self.entries.remove(&old) {
                self.bytes = self.bytes.saturating_sub(t.size()[0] * t.size()[1] * 4);
            }
        }
    }

    /// F5, or artwork changed in Properties: everything is read again.
    pub fn invalidate(&mut self, epoch: u64) {
        if epoch != self.epoch {
            (self.epoch, self.bytes) = (epoch, 0);
            self.entries.clear();
            self.order.clear();
        }
    }
}

/// The part of an image of `tex` size that fills `target` cropped to its aspect, as UVs (crop-to-fill).
pub fn crop_uv(tex: egui::Vec2, target: egui::Vec2) -> egui::Rect {
    let (tw, th) = (tex.x.max(1.0), tex.y.max(1.0));
    let factor = (target.x / tw).max(target.y / th).max(f32::EPSILON);
    let (sw, sh) = (target.x / factor, target.y / factor);
    let (u0, v0) = ((tw - sw) / 2.0 / tw, (th - sh) / 2.0 / th);
    egui::Rect::from_min_max(egui::pos2(u0, v0), egui::pos2(1.0 - u0, 1.0 - v0))
}
