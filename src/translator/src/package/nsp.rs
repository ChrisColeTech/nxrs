//! A debugging listing of a package's members and NCAs (NSP, XCI, bare NCA).
use crate::package::cnmt;
use crate::package::container;
use crate::package::error::Result;
use crate::package::io::{hex, Blob};
use crate::package::keys::Keys;
use crate::package::nca::{content_type_name, open_ncas};
use std::sync::Arc;

pub fn list(blob: Arc<Blob>, keys: &Keys) -> Result<Vec<String>> {
    let members = container::members(&blob)?;
    let (ncas, locked) = open_ncas(&blob, keys)?;
    let mut out = vec![format!(
        "{} ({} bytes, {} members, {locked} NCA header(s) not opened)",
        blob.path.display(),
        blob.size,
        members.len()
    )];
    for m in &members {
        let mut line = format!("  {:<48} {:>12}", m.name, m.size);
        if let Some(n) = ncas.iter().find(|n| n.name == m.name) {
            line += &format!(
                "  {} title {:016x} gen {:02x} {}{}",
                content_type_name(n.content_type),
                n.title_id,
                n.key_generation,
                if n.rights.iter().any(|&b| b != 0) {
                    format!("rights {} ", hex(&n.rights))
                } else {
                    String::new()
                },
                n.key_status()
            );
            for s in &n.sections {
                line += &format!(
                    " [s{} {} crypto {} {:#x}]",
                    s.index,
                    s.kind(),
                    s.crypto,
                    s.size
                );
            }
            if n.content_type == 1 {
                if let Some(s) = n.sections.first() {
                    for (name, data) in crate::package::app::pfs_files(n, s) {
                        if let Ok(c) = cnmt::parse(&data) {
                            line += &format!(
                                "\n      {name}: {} {:016x} v{}",
                                cnmt::kind_name(c.kind),
                                c.title_id,
                                c.version
                            );
                        }
                    }
                }
            }
        }
        out.push(line);
    }
    Ok(out)
}
