//! The readers, and how a file's bytes reach them: by content, never by name. A loose file is decompressed by its
//! magic (zstd, gzip, Yaz0, size-prefixed frames), then archives are opened member by member (SARC, GFPAK) and every
//! shader container found inside is read.

pub mod archive;
pub mod engines;
pub mod gamefreak;
pub mod glslc;
pub mod nso;
pub mod nvnfmt;
pub mod zbic;

use crate::nvn::*;

/// Archives inside archives a scan opens (real files nest a few).
pub const NEST: usize = 16;

/// What the readers share for one game: its zstd dictionaries.
#[derive(Default)]
pub struct Ctx {
    pub dicts: archive::ZstdDicts,
}

/// What one file gave: its shaders and the reasons any part of it could not be read.
#[derive(Default)]
pub struct Scanned {
    pub found: Vec<Found>,
    pub problems: Vec<String>,
}

/// The markers that send bytes to the readers; bytes with none of them hold no shader this scan can read.
fn worth_reading(d: &[u8]) -> bool {
    let head4 = d.get(..4).unwrap_or(&[]);
    use memchr::memmem::find;
    find(d, b"BNSH\0\0\0\0").is_some()
        || find(d, b"BAHS").is_some()
        || matches!(head4, b"MSHD" | b"NDSR" | b"NDVE")
        || (head4 == b"SARC" && (find(d, b"Yaz0").is_some() || find(d, &archive::ZSTD_MAGIC).is_some()))
        || find(d, &glslc::MAGIC).is_some()
        || (d.len() < 0x40000 && !nvnfmt::sead_prim(d, "").is_empty())
        || d.get(..8) == Some(&engines::PACX4[..])
        || find(d, engines::NEEDLE).is_some()
}

/// A container's bytes: archives opened, every shader container read. Errors leave out the part that failed.
pub fn scan_blob(ctx: &Ctx, d: &[u8], label: &str, out: &mut Scanned, depth: usize) {
    if depth > NEST {
        out.problems.push(format!("{label}: archives nested more than {NEST} deep"));
        return;
    }
    let head = |n: usize| d.get(..n).unwrap_or(&[]);
    let r = (|| -> R<()> {
        if head(4) == b"SHN!" {
            out.found.extend(nvnfmt::shn(d, label)?);
            return Ok(());
        }
        if head(4) == archive::ZSTD_MAGIC {
            let dec = archive::unzstd(d, &ctx.dicts, label)?;
            scan_blob(ctx, &dec, label, out, depth + 1);
            return Ok(());
        }
        if head(8) == engines::PACX4 {
            for (nm, p) in engines::pacx4_parts(d, label)? {
                scan_blob(ctx, &p, &format!("{label}/{nm}"), out, depth + 1);
            }
            return Ok(());
        }
        if head(4) == b"Yaz0" {
            let dec = archive::yaz0(d, label)?;
            scan_blob(ctx, &dec, label, out, depth + 1);
            return Ok(());
        }
        if head(4) == b"SARC" {
            for (nm, m) in archive::sarc_members(d, label)? {
                scan_blob(ctx, m, &format!("{label}/{nm}"), out, depth + 1);
            }
            return Ok(());
        }
        if head(8) == gamefreak::GFPAK {
            gamefreak::gfpak_members(d, label, &mut |i, m| scan_blob(ctx, &m, &format!("{label}#{i}"), out, depth + 1))?;
            return Ok(());
        }
        if head(4) == b"BAHS" {
            out.found.extend(nvnfmt::bahs(d, label)?);
            return Ok(());
        }
        if head(4) == b"NDSR" {
            out.found.extend(nvnfmt::ndsr(d, label));
            return Ok(());
        }
        if head(4) == b"NDVE" {
            out.found.extend(nvnfmt::ndve(d, label));
            return Ok(());
        }
        let prim = nvnfmt::sead_prim(d, label);
        if !prim.is_empty() {
            out.found.extend(prim);
            return Ok(());
        }
        if head(4) == b"MSHD" {
            out.found.extend(nvnfmt::mshd(d, label)?);
            return Ok(());
        }
        nvnfmt::bnsh_all(d, label, &mut out.found)?;
        if memchr::memmem::find(d, engines::NEEDLE).is_some() {
            out.found.extend(engines::needle(d, label)?);
        }
        Ok(())
    })();
    if let Err(e) = r {
        out.problems.push(e);
    }
}

/// A file's bytes as stored (loose in the RomFS, or a file inside a pack): decompressed by its magic, then read.
pub fn scan_file(ctx: &Ctx, data: &[u8], label: &str, out: &mut Scanned) {
    let mut d = std::borrow::Cow::Borrowed(data);
    let head = d.get(..16).unwrap_or(&d[..]).to_vec();
    let r = (|| -> R<()> {
        if head.get(..8) == Some(&gamefreak::GFPAK[..]) || head.get(..4) == Some(b"SHN!") {
            scan_blob(ctx, &d, label, out, 0);
            return Ok(());
        }
        if head.get(..4) == Some(b"BERG") {
            out.found.extend(engines::snowdrop(&d, label, false)?);
            return Ok(());
        }
        if head.get(..4) == Some(&archive::ZSTD_MAGIC[..]) {
            d = archive::unzstd(&d, &ctx.dicts, label)?.into();
        } else if head.get(4..8) == Some(&archive::ZSTD_MAGIC[..]) || head.get(4..8) == Some(&archive::LZ4_FRAME_MAGIC[..]) {
            d = archive::size_prefixed(&d, &ctx.dicts, label, &mut out.problems).into_owned().into();
        }
        if d.get(..3) == Some(&[0x1f, 0x8b, 8][..]) {
            d = archive::ungzip(&d, label)?.into();
        }
        if d.get(..4) == Some(b"Yaz0") {
            d = archive::yaz0(&d, label)?.into();
        }
        if d.get(..8) == Some(&gamefreak::GFPAK[..]) || d.get(..4) == Some(b"SHN!") {
            scan_blob(ctx, &d, label, out, 0);
            return Ok(());
        }
        if !worth_reading(&d) {
            return Ok(());
        }
        let before = out.found.len();
        scan_blob(ctx, &d, label, out, 0);
        // whole GLSLC outputs in a file no other reader takes
        if out.found.len() == before && d.get(..4) != Some(b"MSHD") {
            out.found.extend(glslc::shaders(&d, label));
        }
        Ok(())
    })();
    if let Err(e) = r {
        out.problems.push(e);
    }
}

/// A code module's image (main, sdk, subsdk*): containers anywhere in it, and loose GLSLC outputs.
pub fn scan_code(ctx: &Ctx, image: &[u8], label: &str, out: &mut Scanned) {
    scan_blob(ctx, image, label, out, 0);
    out.found.extend(glslc::shaders(image, label));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::archive::tests::{make_sarc, make_yaz0};
    use crate::formats::nvnfmt::tests::make_bnsh;
    use crate::nvn::synth::*;

    #[test]
    fn nested_containers() {
        // zstd(SARC{ a.bfsha = BNSH, b.szs = Yaz0(SARC{ c.bfsha = BNSH }) })
        let inner = make_sarc(&[("c.bfsha", &make_bnsh(nvn1_vs(9), nvn1_ps(9)))]);
        let outer = make_sarc(&[("a.bfsha", &make_bnsh(nvn1_vs(1), nvn1_ps(2))), ("b.szs", &make_yaz0(&inner))]);
        let file = zstd::bulk::compress(&outer, 3).unwrap();
        let mut s = Scanned::default();
        scan_file(&Ctx::default(), &file, "/Shader/x.pack.zs", &mut s);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        let labels: Vec<&str> = s.found.iter().map(|f| f.label.as_str()).collect();
        assert_eq!(labels, vec!["/Shader/x.pack.zs/a.bfsha#BNSH@0x0#var0", "/Shader/x.pack.zs/a.bfsha#BNSH@0x0#var0", "/Shader/x.pack.zs/b.szs/c.bfsha#BNSH@0x0#var0", "/Shader/x.pack.zs/b.szs/c.bfsha#BNSH@0x0#var0"]);
    }

    #[test]
    fn unreadable_part_is_reported() {
        let mut bad = make_bnsh(nvn1_vs(1), nvn1_ps(2));
        let n = bad.len();
        bad[0x1c..0x20].copy_from_slice(&(n as u32).to_le_bytes());
        // point the pixel stage's code past the end
        bad[0x1b0..0x1b8].copy_from_slice(&(n as u64).to_le_bytes());
        let mut s = Scanned::default();
        scan_file(&Ctx::default(), &bad, "/bad.bfsha", &mut s);
        assert!(s.found.is_empty());
        assert_eq!(s.problems.len(), 1);
    }

    #[test]
    fn files_without_markers_are_skipped() {
        let mut s = Scanned::default();
        scan_file(&Ctx::default(), b"just some texture data, no shader here", "/t.bntx", &mut s);
        assert!(s.found.is_empty() && s.problems.is_empty());
    }
}
