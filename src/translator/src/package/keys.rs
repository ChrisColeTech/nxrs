//! prod.keys parsing and header-key derivation.
//! Key values are never printed.
use crate::package::crypto::ecb_decrypt;
use crate::package::error::{PackageError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Keys {
    /// The keys file (absolute), recorded in the `romfs.nsp` marker so the runtime finds it again.
    pub path: PathBuf,
    values: HashMap<String, Vec<u8>>,
}

impl std::fmt::Debug for Keys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Keys({} entries)", self.values.len())
    }
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.is_empty() || s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

impl Keys {
    pub fn parse(text: &str) -> Keys {
        let mut values = HashMap::new();
        for line in text.lines() {
            let Some((name, value)) = line.split_once('=') else { continue };
            if let Some(v) = unhex(value) {
                values.insert(name.trim().to_ascii_lowercase(), v);
            }
        }
        Keys { path: PathBuf::new(), values }
    }

    pub fn load(path: &Path) -> Result<Keys> {
        let bytes = std::fs::read(path).map_err(|source| PackageError::Io { path: path.to_path_buf(), source })?;
        let mut keys = Keys::parse(&String::from_utf8_lossy(&bytes));
        keys.path = crate::package::io::abspath(path);
        Ok(keys)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[allow(dead_code)]
    pub fn has(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    pub fn get16(&self, name: &str) -> Result<[u8; 16]> {
        Ok(self.get(name, 16)?[..].try_into().unwrap())
    }

    /// nca.py `Keys.get`: the named key of `size` bytes, or the reason it is missing.
    pub fn get(&self, name: &str, size: usize) -> Result<Vec<u8>> {
        match self.values.get(name) {
            Some(v) if v.len() == size => Ok(v.clone()),
            _ => Err(PackageError::Format(format!(
                "the installed keys have no {name} (dump prod.keys again from a console with current firmware)"
            ))),
        }
    }

    pub fn header_key(&self) -> Result<[u8; 32]> {
        if let Some(v) = self.values.get("header_key") {
            if v.len() == 32 {
                return Ok(v[..].try_into().unwrap());
            }
        }
        let to16 = |v: Vec<u8>| -> [u8; 16] { v[..].try_into().unwrap() };
        let kek = to16(ecb_decrypt(&self.get16("master_key_00")?, &self.get16("aes_kek_generation_source")?));
        let kek = to16(ecb_decrypt(&ecb_decrypt(&kek, &self.get16("header_kek_source")?).try_into().unwrap(), &self.get16("aes_key_generation_source")?));
        Ok(ecb_decrypt(&kek, &self.get("header_key_source", 32)?)[..].try_into().unwrap())
    }
}

/// nsp.py `_keys_path`: the explicit path, else SWR_PROD_KEYS, else the launcher's copy; a missing file is the
/// error Turbo gives.
pub fn keys_path(explicit: Option<&Path>) -> Result<PathBuf> {
    let p = explicit.map(Path::to_path_buf).or_else(default_keys_path);
    match p {
        Some(p) if p.is_file() => Ok(crate::package::io::abspath(&p)),
        _ => Err(PackageError::Format(
            "Nintendo keys are not installed. In the launcher use Keys > Install keys, or set SWR_PROD_KEYS.".into(),
        )),
    }
}

/// env SWR_PROD_KEYS, else %LOCALAPPDATA%\RecompLauncher\RecompLauncher\keys\prod.keys if it exists.
pub fn default_keys_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SWR_PROD_KEYS") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    let local = std::env::var_os("LOCALAPPDATA")?;
    let p = PathBuf::from(local).join("RecompLauncher").join("RecompLauncher").join("keys").join("prod.keys");
    p.is_file().then_some(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_keys_text() {
        let text = "header_key = 000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\r\n\
                    ; comment line\n\
                    TitleKek_00=00112233445566778899AABBCCDDEEFF\n\
                    bad = zz\n\
                    odd = 123\n\
                    key_area_key_application_0a = ffeeddccbbaa99887766554433221100\n";
        let k = Keys::parse(text);
        assert_eq!(k.len(), 3);
        assert_eq!(k.header_key().unwrap()[31], 0x1f);
        assert_eq!(k.get16("titlekek_00").unwrap()[15], 0xff);
        assert_eq!(k.get16("key_area_key_application_0a").unwrap()[0], 0xff);
        assert!(k.get16("titlekek_01").is_err());
        assert!(!k.has("bad") && !k.has("odd"));
        assert!(!format!("{k:?}").contains("0011"));
    }
}
