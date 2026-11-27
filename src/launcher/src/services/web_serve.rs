//! The local web server that plays web builds (docs/35 W5): HTTP/1.1 on 127.0.0.1 in the launcher's
//! own process, one thread per connection. Every built game is mounted under `/<slug>/`; one fixed
//! port keeps the page's origin, and with it the browser's data cache and saves, from one Play to the
//! next. What each answer carries is `web_reply`'s.

use super::files::{is_dir, is_file, list, read_text};
use crate::constants::{web as c, words_web as w};
use crate::model::fill::fill;
use crate::model::game::Game;
use crate::model::path_text::{join, native, parent};
use crate::model::web_build::{self, WebBuild, web_dir};
use std::collections::HashMap;
use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// A served build: its folder, its data folder, its page.
#[derive(Clone, Debug, PartialEq)]
pub struct Mount {
    pub root: PathBuf,
    pub data: PathBuf,
    pub entry: String,
    /// the nxrs-build that translates the page's run-time shaders (`web_xlate`); empty: none
    pub translator: String,
}

pub type Mounts = Arc<Mutex<HashMap<String, Mount>>>;

pub struct WebServer {
    port: u16,
    mounts: Mounts,
    /// the port asked for, when another one was taken instead
    pub moved_from: Option<u16>,
}

/// A web build folder read for serving: its `build.json`, its data folder there.
pub fn open_build(dir: &str) -> Result<(WebBuild, Mount), String> {
    let text = read_text(&join(dir, c::BUILD_JSON)).ok_or_else(|| w::NO_BUILD_JSON.to_string())?;
    let b = web_build::parse(&text)?;
    let data = join(dir, &b.data);
    if !is_dir(&data) {
        return Err(format!("{}{}", w::NO_DATA_DIR, native(&data)));
    }
    let mount = Mount {
        root: PathBuf::from(native(dir)),
        data: PathBuf::from(native(&data)),
        entry: b.entry.clone(),
        translator: String::new(),
    };
    Ok((b, mount))
}

/// The folder holds a finished web build (`build.json` and the module): Play in browser is offered.
pub fn has_web_build(dir: &str) -> bool {
    !dir.is_empty() && is_file(&join(dir, c::BUILD_JSON)) && is_file(&join(dir, c::MODULE))
}

/// The web build Play serves: the game's own web folder when it holds one, else a build beside it
/// made under another name (a CLI build without `-o` names its folder after the package, not the
/// library's output name) whose `build.json` carries the game's title id; else the own folder.
pub fn played_web_dir(game: &Game) -> String {
    // an imported nxrs web build (a web game's page is `web_build::imported_page`'s)
    if !game.web_build.is_empty() && web_build::imported_page(game).is_none() {
        return game.web_build.clone();
    }
    let own = web_dir(game);
    if has_web_build(&own) {
        return own;
    }
    let Some(id) = super::kept_saves::title_id(&game.data_dir()) else {
        return own;
    };
    let base = parent(&own);
    list(&base)
        .into_iter()
        .filter(|(_, folder)| *folder)
        .map(|(name, _)| join(&base, &name))
        .find(|dir| has_web_build(dir) && build_title_id(dir).as_deref() == Some(id.as_str()))
        .unwrap_or(own)
}

/// The entry can play in the browser: its imported web game's page is there, else its web build is.
pub fn can_serve(game: &Game) -> bool {
    match web_build::imported_page(game) {
        Some(page) => is_file(page),
        None => has_web_build(&played_web_dir(game)),
    }
}

/// An imported web game served from its page's folder, the page at the folder's address.
pub fn open_site(page: &str) -> Result<Mount, String> {
    if !is_file(page) {
        return Err(format!("{}{}", w::NO_PAGE, native(page)));
    }
    let root = PathBuf::from(native(&parent(page)));
    Ok(Mount {
        data: root.join(c::DATA_SEGMENT),
        root,
        entry: crate::model::path_text::file_name(page),
        translator: String::new(),
    })
}

/// `build.json`'s `titleId`, lowercase.
pub fn build_title_id(dir: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&read_text(&join(dir, c::BUILD_JSON))?).ok()?;
    Some(v.get("titleId")?.as_str()?.to_ascii_lowercase())
}

/// The port asked for: `NXRS_WEB_PORT`, else the default.
pub fn wanted_port() -> u16 {
    std::env::var(c::ENV_PORT)
        .ok()
        .and_then(|p| p.trim().parse().ok())
        .unwrap_or(c::DEFAULT_PORT)
}

impl WebServer {
    /// Listens on `port` (0: any), else the next free one of `PORT_TRIES`; serves until the process ends.
    pub fn start(port: u16) -> Result<WebServer, String> {
        let tries = if port == 0 { 1 } else { c::PORT_TRIES };
        let listener = (0..tries)
            .find_map(|i| TcpListener::bind((Ipv4Addr::LOCALHOST, port.saturating_add(i))).ok())
            .ok_or_else(|| format!("{}{}:{port}", w::NO_SERVER, c::HOST))?;
        let bound = listener
            .local_addr()
            .map_err(|e| format!("{}{e}", w::NO_SERVER))?
            .port();
        let mounts: Mounts = Arc::default();
        let shared = mounts.clone();
        std::thread::Builder::new()
            .name("nxrs-web-serve".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let m = shared.clone();
                    let _ = std::thread::Builder::new()
                        .name("nxrs-web-conn".into())
                        .spawn(move || super::web_reply::serve(stream, &m));
                }
            })
            .map_err(|e| format!("{}{e}", w::NO_SERVER))?;
        let moved_from = (port != 0 && bound != port).then_some(port);
        Ok(WebServer {
            port: bound,
            mounts,
            moved_from,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Serves `mount` under the slug of `name` (replacing an earlier mount of it); the page's address.
    pub fn mount(&self, name: &str, mount: Mount) -> Result<String, String> {
        let slug = web_build::slug(name);
        let mut m = self
            .mounts
            .lock()
            .map_err(|_| format!("{}{}", w::NO_SERVER, w::NOT_SERVED_TITLE))?;
        m.insert(slug.clone(), mount);
        Ok(web_build::url(self.port, &slug))
    }

    /// Serves a web game at the root of its own `<slug>.localhost` (replacing an earlier mount of
    /// it); its address.
    pub fn mount_site(&self, name: &str, mount: Mount) -> Result<String, String> {
        let slug = web_build::slug(name);
        let mut m = self
            .mounts
            .lock()
            .map_err(|_| format!("{}{}", w::NO_SERVER, w::NOT_SERVED_TITLE))?;
        m.insert(super::web_route::site_key(&slug), mount);
        Ok(web_build::site_url(self.port, &slug))
    }

    /// The line said when the port asked for was taken.
    pub fn moved_note(&self) -> Option<String> {
        self.moved_from
            .map(|p| fill(w::OTHER_PORT, &[&p.to_string(), &self.port.to_string()]))
    }
}
