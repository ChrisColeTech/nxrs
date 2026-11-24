//! Game-data-free tests of the whole package path with test keys: an AES-CTR base Program NCA with a common
//! ticket, an update whose RomFS is a BKTR patch over the base's (key area key, counter generations changing in the
//! middle of an AES block), Control and Meta NCAs, add-on content; combined, separate (NSP, XCI, bare NCA), the
//! markers, update-info, the refusals and DLC install. The fixture follows the owner's tests/test_nsp_combined.py.
use crate::package::container::build_xci;
use crate::package::crypto::{ctr, ecb_encrypt, xts};
use crate::package::pfs0::build as pfs;
use crate::package::romfs::build_image as romfs_image;
use crate::package::{dlc, marker, open_game, update_info, Applied, OpenOptions, UpdateChoice};
use std::path::{Path, PathBuf};

const GAME: u64 = 0x0100_AAAA_0000_C000;

fn header_key() -> [u8; 32] {
    std::array::from_fn(|i| i as u8)
}
fn titlekek() -> [u8; 16] {
    std::array::from_fn(|i| 0x40 + i as u8)
}
fn area_kek() -> [u8; 16] {
    std::array::from_fn(|i| 0x60 + i as u8)
}
fn shared() -> Vec<u8> {
    (0..0x9000usize)
        .map(|i| ((i * 7 + i / 251) & 0xff) as u8)
        .collect()
}

/// (generation, secure value) as the FS header stores them; upper = secure BE || generation BE.
fn upper(counter: (u32, u32), generation: Option<u32>) -> [u8; 8] {
    let mut u = [0u8; 8];
    u[..4].copy_from_slice(&counter.1.to_be_bytes());
    u[4..].copy_from_slice(&generation.unwrap_or(counter.0).to_be_bytes());
    u
}

fn fs_header(
    pfs_kind: bool,
    crypto: u8,
    counter: (u32, u32),
    data_at: u64,
    data_size: u64,
) -> Vec<u8> {
    let mut fs = vec![0u8; 0x200];
    fs[0] = 2;
    if pfs_kind {
        fs[2..5].copy_from_slice(&[1, 2, crypto]);
        fs[0x40..0x48].copy_from_slice(&data_at.to_le_bytes());
        fs[0x48..0x50].copy_from_slice(&data_size.to_le_bytes());
    } else {
        fs[2..5].copy_from_slice(&[0, 3, crypto]);
        fs[8..12].copy_from_slice(b"IVFC");
        let at = 0x18 + 5 * 0x18;
        fs[at..at + 8].copy_from_slice(&data_at.to_le_bytes());
        fs[at + 8..at + 16].copy_from_slice(&data_size.to_le_bytes());
        fs[at + 16..at + 20].copy_from_slice(&14u32.to_le_bytes());
    }
    fs[0x140..0x144].copy_from_slice(&counter.0.to_le_bytes());
    fs[0x144..0x148].copy_from_slice(&counter.1.to_le_bytes());
    fs
}

type Seal = Box<dyn Fn(u64, &[u8]) -> Vec<u8>>;

fn plain_ctr(key: [u8; 16], counter: (u32, u32)) -> Option<Seal> {
    Some(Box::new(move |at, data| {
        let mut d = data.to_vec();
        ctr(&key, &upper(counter, None), at, &mut d);
        d
    }))
}

fn build_nca(
    content_type: u8,
    title_id: u64,
    sections: Vec<(Vec<u8>, Vec<u8>, Option<Seal>)>,
    key: [u8; 16],
    rights: Option<[u8; 16]>,
) -> Vec<u8> {
    let mut header = vec![0u8; 0xc00];
    header[0x200..0x204].copy_from_slice(b"NCA3");
    header[0x205] = content_type;
    header[0x206] = 2;
    header[0x220] = 3;
    header[0x210..0x218].copy_from_slice(&title_id.to_le_bytes());
    match rights {
        Some(r) => header[0x230..0x240].copy_from_slice(&r),
        None => header[0x320..0x330].copy_from_slice(&ecb_encrypt(&area_kek(), &key)),
    }
    let mut body = vec![];
    let mut at = 0xc00u64;
    for (i, (fs, plain, seal)) in sections.into_iter().enumerate() {
        let mut plain = plain;
        plain.resize(plain.len().div_ceil(0x200) * 0x200, 0);
        header[0x240 + i * 0x10..0x244 + i * 0x10]
            .copy_from_slice(&((at / 0x200) as u32).to_le_bytes());
        header[0x244 + i * 0x10..0x248 + i * 0x10]
            .copy_from_slice(&(((at + plain.len() as u64) / 0x200) as u32).to_le_bytes());
        header[0x400 + i * 0x200..0x600 + i * 0x200].copy_from_slice(&fs);
        body.extend(match seal {
            Some(s) => s(at, &plain),
            None => plain.clone(),
        });
        at += plain.len() as u64;
    }
    header[0x208..0x210].copy_from_slice(&at.to_le_bytes());
    xts(&header_key(), &mut header, 0, 0x200, true);
    header.extend(body);
    header
}

fn ticket(rights: [u8; 16], key: [u8; 16], personalized: bool) -> Vec<u8> {
    let mut d = vec![0u8; 0x2c0];
    d[..4].copy_from_slice(&0x10004u32.to_le_bytes());
    d[0x180..0x190].copy_from_slice(&ecb_encrypt(&titlekek(), &key));
    d[0x281] = personalized as u8;
    d[0x2a0..0x2b0].copy_from_slice(&rights);
    d
}

fn id(byte: &str) -> [u8; 16] {
    let b = u8::from_str_radix(byte, 16).unwrap();
    [b; 16]
}

fn meta_nca(
    title_id: u64,
    version: u32,
    kind: u8,
    records: &[(&str, u8)],
    key: [u8; 16],
) -> Vec<u8> {
    let ids: Vec<([u8; 16], u8)> = records.iter().map(|(i, t)| (id(i), *t)).collect();
    let refs: Vec<(&[u8; 16], u8)> = ids.iter().map(|(i, t)| (i, *t)).collect();
    let image = pfs(&[(
        "meta.cnmt",
        &crate::package::cnmt::build(title_id, version, kind, &refs),
    )]);
    build_nca(
        1,
        title_id,
        vec![(
            fs_header(true, 3, (0, 0), 0, image.len() as u64),
            image,
            plain_ctr(key, (0, 0)),
        )],
        key,
        None,
    )
}

fn control_nca(title_id: u64, display: &str, key: [u8; 16]) -> Vec<u8> {
    let mut nacp = vec![0u8; 0x4000];
    nacp[0x3060..0x3060 + display.len()].copy_from_slice(display.as_bytes());
    let icon = [b"\xff\xd8icon".as_slice(), display.as_bytes()].concat();
    let image = romfs_image(&[("control.nacp", &nacp), ("icon_AmericanEnglish.dat", &icon)]);
    let mut sec = vec![0u8; 0x200];
    sec.extend(&image);
    build_nca(
        2,
        title_id,
        vec![(
            fs_header(false, 3, (0, 5), 0x200, image.len() as u64),
            sec,
            plain_ctr(key, (0, 5)),
        )],
        key,
        None,
    )
}

fn nso(tag: &[u8]) -> Vec<u8> {
    let mut n = b"NSO0".to_vec();
    n.resize(0x100, 0);
    n.extend(tag);
    n
}

struct Fixture {
    base_key: [u8; 16],
    update_key: [u8; 16],
    control_key: [u8; 16],
    rights: [u8; 16],
    base_image: Vec<u8>,
    new_image: Vec<u8>,
}

type Files = Vec<(String, Vec<u8>)>;

fn find(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).position(|w| w == needle).unwrap()
}

impl Fixture {
    fn new() -> Fixture {
        let mut rights = [0u8; 16];
        rights[..8].copy_from_slice(&GAME.to_be_bytes());
        rights[15] = 3;
        let new_bin: Vec<u8> = (0..9).flat_map(|_| 0..=255u8).collect();
        Fixture {
            base_key: std::array::from_fn(|i| 0x10 + i as u8),
            update_key: std::array::from_fn(|i| 0x80 + i as u8),
            control_key: std::array::from_fn(|i| 0xa0 + i as u8),
            rights,
            base_image: romfs_image(&[
                ("keep.bin", &shared()),
                ("file.bin", b"test"),
                ("gone.bin", b"removed by the update"),
            ]),
            new_image: romfs_image(&[
                ("keep.bin", &shared()),
                ("file.bin", b"patched!"),
                ("new.bin", &new_bin),
            ]),
        }
    }

    fn base(&self, personalized: bool) -> Files {
        let main = nso(b"base");
        let exefs = pfs(&[("main", &main), ("main.npdm", b"npdm")]);
        let mut sec = vec![0u8; 0x400];
        sec.extend(&self.base_image);
        let program = build_nca(
            0,
            GAME,
            vec![
                (
                    fs_header(true, 3, (0, 1), 0, exefs.len() as u64),
                    exefs,
                    plain_ctr(self.base_key, (0, 1)),
                ),
                (
                    fs_header(false, 3, (0, 2), 0x400, self.base_image.len() as u64),
                    sec,
                    plain_ctr(self.base_key, (0, 2)),
                ),
            ],
            self.base_key,
            Some(self.rights),
        );
        vec![
            (format!("{}.nca", "aa".repeat(16)), program),
            (
                format!("{}.nca", "ab".repeat(16)),
                control_nca(GAME, "1.0.0", self.control_key),
            ),
            (
                format!("{}.cnmt.nca", "ac".repeat(16)),
                meta_nca(GAME, 0, 0x80, &[("aa", 1), ("ab", 3)], self.control_key),
            ),
            (
                format!("{}.tik", crate::package::io::hex(&self.rights)),
                ticket(self.rights, self.base_key, personalized),
            ),
        ]
    }

    /// The patch section: [hash area + the new image's head][the new image's tail][indirect][AesCtrEx].
    fn update(&self, title: u64) -> Files {
        let counter = (3u32, 2u32);
        let sh = shared();
        let base_at = 0x400 + find(&self.base_image, &sh) as u64;
        let shared_at = find(&self.new_image, &sh);
        let mut head = vec![0u8; 0x800];
        head.extend(&self.new_image[..shared_at]);
        let tail = &self.new_image[shared_at + sh.len()..];
        let mut physical = head.clone();
        physical.extend(tail);
        physical.resize(physical.len().div_ceil(16) * 16, 0);
        let hl = head.len() as u64;
        let relocations = [
            (0u64, 0u64, 1u32),
            (hl, base_at, 0),
            (hl + sh.len() as u64, hl, 1),
        ];
        let virtual_size = 0x800 + self.new_image.len() as u64;
        let indirect_at = physical.len() as u64;
        let table_size = 0x8000u64;
        let ex_at = indirect_at + table_size;
        let generations = [
            (0u64, 5u32),
            (0x813, 7),
            (hl + 5, 9),
            (indirect_at, counter.0),
        ];
        let table = |entries: Vec<Vec<u8>>, total: u64| {
            let mut h = vec![0u8; 0x4000];
            h[4..8].copy_from_slice(&1u32.to_le_bytes());
            h[8..16].copy_from_slice(&total.to_le_bytes());
            let mut b = vec![0u8; 0x10];
            b[4..8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
            b[8..16].copy_from_slice(&total.to_le_bytes());
            for e in entries {
                b.extend(e);
            }
            b.resize(0x4000, 0);
            h.extend(b);
            h
        };
        let mut section = physical.clone();
        section.extend(table(
            relocations
                .iter()
                .map(|(v, p, s)| {
                    [
                        v.to_le_bytes().to_vec(),
                        p.to_le_bytes().to_vec(),
                        s.to_le_bytes().to_vec(),
                    ]
                    .concat()
                })
                .collect(),
            virtual_size,
        ));
        section.extend(table(
            generations
                .iter()
                .map(|(a, g)| {
                    [
                        a.to_le_bytes().to_vec(),
                        0u32.to_le_bytes().to_vec(),
                        g.to_le_bytes().to_vec(),
                    ]
                    .concat()
                })
                .collect(),
            indirect_at,
        ));
        let key = self.update_key;
        let gens = generations;
        let seal: Seal = Box::new(move |at, data| {
            let mut out = vec![];
            for (i, (start, g)) in gens.iter().enumerate() {
                let end = gens.get(i + 1).map_or(data.len() as u64, |x| x.0);
                let mut part = data[*start as usize..end as usize].to_vec();
                ctr(&key, &upper(counter, Some(*g)), at + start, &mut part);
                out.extend(part);
            }
            out
        });
        let mut fs = fs_header(false, 4, counter, 0x800, self.new_image.len() as u64);
        fs[0x100..0x108].copy_from_slice(&indirect_at.to_le_bytes());
        fs[0x108..0x110].copy_from_slice(&table_size.to_le_bytes());
        fs[0x110..0x114].copy_from_slice(b"BKTR");
        fs[0x120..0x128].copy_from_slice(&ex_at.to_le_bytes());
        fs[0x128..0x130].copy_from_slice(&table_size.to_le_bytes());
        fs[0x130..0x134].copy_from_slice(b"BKTR");
        let main = nso(b"update");
        let exefs = pfs(&[("main", &main), ("main.npdm", b"npdm2")]);
        let program = build_nca(
            0,
            title,
            vec![
                (
                    fs_header(true, 3, (3, 1), 0, exefs.len() as u64),
                    exefs,
                    plain_ctr(key, (3, 1)),
                ),
                (fs, section, Some(seal)),
            ],
            key,
            None,
        );
        vec![
            (format!("{}.nca", "ba".repeat(16)), program),
            (
                format!("{}.nca", "bb".repeat(16)),
                control_nca(title, "1.2.3", self.control_key),
            ),
            (
                format!("{}.cnmt.nca", "bc".repeat(16)),
                meta_nca(
                    title + 0x800,
                    262144,
                    0x81,
                    &[("ba", 1), ("bb", 3)],
                    self.control_key,
                ),
            ),
        ]
    }

    /// An add-on title with its data (a RomFS, AES-CTR with the key area key).
    fn addon(&self) -> Files {
        let image = romfs_image(&[("contents.dat", &[7u8; 4096]), ("version.txt", b"10002")]);
        let mut sec = vec![0u8; 0x200];
        sec.extend(&image);
        let data = build_nca(
            4,
            GAME + 0x1001,
            vec![(
                fs_header(false, 3, (0, 9), 0x200, image.len() as u64),
                sec,
                plain_ctr(self.control_key, (0, 9)),
            )],
            self.control_key,
            None,
        );
        vec![
            (
                format!("{}.cnmt.nca", "cc".repeat(16)),
                meta_nca(GAME + 0x1001, 65536, 0x82, &[("cd", 2)], self.control_key),
            ),
            (format!("{}.nca", "cd".repeat(16)), data),
        ]
    }
}

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn setup(tag: &str) -> (Tmp, PathBuf) {
    let root = std::env::temp_dir().join(format!("nxrs-synth-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let keys = root.join("prod.keys");
    std::fs::write(
        &keys,
        format!(
            "header_key = {}\ntitlekek_02 = {}\nkey_area_key_application_02 = {}\n",
            crate::package::io::hex(&header_key()),
            crate::package::io::hex(&titlekek()),
            crate::package::io::hex(&area_kek())
        ),
    )
    .unwrap();
    (Tmp(root.clone()), keys)
}

fn write_pfs(path: &Path, parts: &[Files]) {
    let all: Vec<(String, Vec<u8>)> = parts.iter().flatten().cloned().collect();
    let refs: Vec<(&str, &[u8])> = all
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    std::fs::write(path, pfs(&refs)).unwrap();
}

fn opts(keys: &Path, update: UpdateChoice) -> OpenOptions {
    OpenOptions {
        keys: Some(keys.to_path_buf()),
        update,
    }
}

fn files(r: &crate::package::RomFs) -> Vec<(String, Vec<u8>)> {
    r.walk()
        .into_iter()
        .map(|(p, _)| (p.clone(), r.read_file(&p).unwrap()))
        .collect()
}

fn expect_new(r: &crate::package::RomFs) {
    let f = files(r);
    let names: Vec<&str> = f.iter().map(|x| x.0.as_str()).collect();
    assert_eq!(names, ["file.bin", "keep.bin", "new.bin"]);
    assert_eq!(f[0].1, b"patched!");
    assert_eq!(f[1].1, shared());
    assert_eq!(f[2].1.len(), 256 * 9);
    // a read across the base/patch boundary of keep.bin and one from inside it
    assert_eq!(
        r.read("keep.bin", 0x1234, 0x10).unwrap(),
        &shared()[0x1234..0x1244]
    );
}

#[test]
fn combined_package_applies_its_update_per_read() {
    let (_t, keys) = setup("combined");
    let fx = Fixture::new();
    let pkg = _t.0.join("Gamé [v262144] (1G+1U+1D).nsp");
    write_pfs(&pkg, &[fx.base(false), fx.update(GAME), fx.addon()]);
    let g = open_game(&pkg, &opts(&keys, UpdateChoice::Embedded)).unwrap();
    assert_eq!(g.applied, Applied::Embedded);
    assert_eq!(g.exefs.files["main"], nso(b"update"));
    assert_eq!(g.exefs.version, Some(262144));
    assert_eq!(g.display_version.as_deref(), Some("1.2.3"));
    assert_eq!(g.addons, [GAME + 0x1001]);
    expect_new(&g.romfs);
    // the marker alone (what the runtime has) reads the same bytes
    let data = _t.0.join("data");
    g.write_data(&data, &|_| {}).unwrap();
    let text = std::fs::read_to_string(data.join("romfs.nsp")).unwrap();
    assert!(
        text.starts_with("SWRNSP2\n")
            && text.contains("\nbase ")
            && text.contains("\npatch ")
            && !text.contains("\nupdate ")
    );
    assert!(text.contains(&format!(
        "titlekek_02:{}",
        crate::package::io::hex(&ecb_encrypt(&titlekek(), &fx.base_key))
    )));
    expect_new(&marker::open_data_dir(&data, Some(&keys)).unwrap());
    assert_eq!(
        std::fs::read(data.join("exefs/main")).unwrap(),
        nso(b"update")
    );
    assert!(std::fs::read(data.join("control.nacp")).unwrap().len() == 0x4000);
    // --no-update: the game as released
    let b = open_game(&pkg, &opts(&keys, UpdateChoice::None)).unwrap();
    assert_eq!(b.applied, Applied::LeftOut);
    assert_eq!(b.exefs.files["main"], nso(b"base"));
    assert_eq!(
        (b.exefs.version, b.display_version.as_deref()),
        (Some(0), Some("1.0.0"))
    );
    let names: Vec<String> = b.romfs.walk().into_iter().map(|x| x.0).collect();
    assert_eq!(names, ["file.bin", "gone.bin", "keep.bin"]);
    assert_eq!(b.romfs.read_file("file.bin").unwrap(), b"test");
}

/// A card image (XCI) as the game's own package, with and without the key area, holding the game and its update.
#[test]
fn game_from_xci() {
    let (_t, keys) = setup("xcigame");
    let fx = Fixture::new();
    let all: Vec<(String, Vec<u8>)> = [fx.base(false), fx.update(GAME)]
        .into_iter()
        .flatten()
        .collect();
    let refs: Vec<(&str, &[u8])> = all
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    for key_area in [false, true] {
        let pkg = _t.0.join(format!("game{}.xci", key_area as u8));
        std::fs::write(&pkg, build_xci(&refs, key_area)).unwrap();
        let g = open_game(&pkg, &opts(&keys, UpdateChoice::Embedded)).unwrap();
        assert_eq!(g.applied, Applied::Embedded);
        assert_eq!(g.exefs.files["main"], nso(b"update"));
        assert_eq!(g.exefs.version, Some(262144));
        expect_new(&g.romfs);
        let data = _t.0.join(format!("data{}", key_area as u8));
        g.write_data(&data, &|_| {}).unwrap();
        expect_new(&marker::open_data_dir(&data, Some(&keys)).unwrap());
        let b = open_game(&pkg, &opts(&keys, UpdateChoice::None)).unwrap();
        assert_eq!(b.exefs.files["main"], nso(b"base"));
    }
}

#[test]
fn separate_update_nsp_xci_and_bare_nca() {
    let (_t, keys) = setup("separate");
    let fx = Fixture::new();
    let game = _t.0.join("game.nsp");
    write_pfs(&game, &[fx.base(false)]);
    let upd = _t.0.join("update.nsp");
    write_pfs(&upd, &[fx.update(GAME)]);
    let xci = _t.0.join("update.xci");
    let u = fx.update(GAME);
    let refs: Vec<(&str, &[u8])> = u.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
    std::fs::write(&xci, build_xci(&refs, true)).unwrap();
    let bare = _t.0.join("bare").join("update.nca");
    std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
    std::fs::write(&bare, &u[0].1).unwrap();
    for (pkg, version) in [(&upd, 262144), (&xci, 262144), (&bare, 0)] {
        let g = open_game(&game, &opts(&keys, UpdateChoice::Package(pkg.clone()))).unwrap();
        assert_eq!(
            g.applied,
            Applied::Separate(crate::package::io::abspath(pkg))
        );
        assert_eq!(g.exefs.files["main"], nso(b"update"));
        assert_eq!(g.exefs.version, Some(version));
        expect_new(&g.romfs);
        let data = _t.0.join("data");
        g.write_data(&data, &|_| {}).unwrap();
        let text = std::fs::read_to_string(data.join("romfs.nsp")).unwrap();
        assert!(text.contains(&format!(
            "\nupdate {}\n",
            crate::package::io::abspath(pkg).display()
        )));
        expect_new(&marker::open_data_dir(&data, Some(&keys)).unwrap());
    }
    // a bare NCA has no metadata: version 0, the control data stays the game's
    let g = open_game(&game, &opts(&keys, UpdateChoice::Package(bare.clone()))).unwrap();
    assert_eq!(g.display_version.as_deref(), Some("1.0.0"));
    // update-info: alone, and over the game
    let info = update_info(&upd, Some(&keys), None).unwrap();
    assert_eq!(
        info.line(),
        format!(
            "UPDATE\t{}\t{GAME:016x}\t262144\t1.2.3",
            crate::package::io::abspath(&upd).display()
        )
    );
    assert_eq!(update_info(&upd, Some(&keys), Some(&game)).unwrap(), info);
    // the marker names the update package; when it is gone the runtime says so
    let data = _t.0.join("data2");
    open_game(&game, &opts(&keys, UpdateChoice::Package(upd.clone())))
        .unwrap()
        .write_data(&data, &|_| {})
        .unwrap();
    std::fs::rename(&upd, _t.0.join("moved.nsp")).unwrap();
    let e = marker::open_data_dir(&data, Some(&keys))
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("is missing") && e.contains("Install Updates"),
        "{e}"
    );
}

#[test]
fn refusals_with_turbos_reasons() {
    let (_t, keys) = setup("refuse");
    let fx = Fixture::new();
    let game = _t.0.join("game.nsp");
    write_pfs(&game, &[fx.base(false)]);
    let g = crate::package::io::abspath(&game).display().to_string();
    // an update of another game
    let other = _t.0.join("other.nsp");
    write_pfs(&other, &[fx.update(GAME + 0x10000)]);
    let o = crate::package::io::abspath(&other).display().to_string();
    let e = open_game(&game, &opts(&keys, UpdateChoice::Package(other.clone())))
        .err()
        .unwrap()
        .to_string();
    assert_eq!(
        e,
        format!(
            "{o}: this update is for {:016x}, not for this game ({GAME:016x})",
            GAME + 0x10000
        )
    );
    // a package without an update
    let e = update_info(&game, Some(&keys), None)
        .unwrap_err()
        .to_string();
    assert_eq!(e, format!("{g}: the package holds no game update"));
    // an update on its own as the game
    let upd = _t.0.join("update.nsp");
    write_pfs(&upd, &[fx.update(GAME)]);
    let e = open_game(&upd, &opts(&keys, UpdateChoice::Embedded))
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.ends_with("this is an update without its game; use a package that holds both"),
        "{e}"
    );
    // a console-bound ticket
    let bound = _t.0.join("bound.nsp");
    write_pfs(&bound, &[fx.base(true)]);
    let e = open_game(&bound, &opts(&keys, UpdateChoice::Embedded))
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains(
            "is bound to one console (a personalized ticket); dump the game with a common ticket"
        ),
        "{e}"
    );
    // a compressed package
    let nsz = _t.0.join("update.nsz");
    write_pfs(&nsz, &[vec![("x.ncz".into(), vec![0u8; 16])]]);
    let e = open_game(&game, &opts(&keys, UpdateChoice::Package(nsz.clone())))
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains("this is a compressed package (NSZ / XCZ). Its data cannot be read in place"),
        "{e}"
    );
    // a missing update package
    let e = update_info(&_t.0.join("nope.nsp"), Some(&keys), Some(&game))
        .unwrap_err()
        .to_string();
    assert!(
        e.ends_with("nope.nsp: the update package is missing"),
        "{e}"
    );
}

#[test]
fn dlc_install_reads_in_place_and_refuses() {
    let (_t, keys) = setup("dlc");
    let fx = Fixture::new();
    let pkg = _t.0.join("combined.nsp");
    write_pfs(&pkg, &[fx.base(false), fx.update(GAME), fx.addon()]);
    let datadir = _t.0.join("Game_data");
    std::fs::create_dir_all(datadir.join("exefs")).unwrap();
    let mut npdm = vec![0u8; 0x100];
    npdm[..4].copy_from_slice(b"META");
    npdm[0x70..0x74].copy_from_slice(&0x80u32.to_le_bytes());
    npdm[0x80..0x84].copy_from_slice(b"ACI0");
    npdm[0x90..0x98].copy_from_slice(&GAME.to_le_bytes());
    std::fs::write(datadir.join("exefs/main.npdm"), &npdm).unwrap();
    let said = std::cell::RefCell::new(vec![]);
    let say = |m: &str| said.borrow_mut().push(m.to_string());
    let (done, errors) = dlc::install(&[pkg.clone()], &datadir, &say, Some(&keys), false).unwrap();
    assert_eq!((done, errors.len()), (vec![GAME + 0x1001], 0));
    let p = crate::package::io::abspath(&pkg).display().to_string();
    assert!(said.borrow().iter().any(|m| m
        == &format!(
            "DLC: installed {:016x} v65536 (read in place from the package) from {p}",
            GAME + 0x1001
        )));
    assert_eq!(
        dlc::read_manifest(&datadir),
        [(GAME + 0x1001, 65536, "data".to_string(), p.clone())]
    );
    let r = dlc::title_romfs(&datadir, GAME + 0x1001, Some(&keys)).unwrap();
    assert_eq!(r.read_file("version.txt").unwrap(), b"10002");
    assert_eq!(r.read_file("contents.dat").unwrap(), [7u8; 4096]);
    // again: nothing to do
    said.borrow_mut().clear();
    dlc::install(&[pkg.clone()], &datadir, &say, Some(&keys), false).unwrap();
    assert_eq!(
        said.borrow()[0],
        format!("DLC: {:016x} v65536 is already installed", GAME + 0x1001)
    );
    // a package without add-on content
    let game_only = _t.0.join("game.nsp");
    write_pfs(&game_only, &[fx.base(false)]);
    let (_, errors) =
        dlc::install(&[game_only.clone()], &datadir, &|_| {}, Some(&keys), false).unwrap();
    assert_eq!(
        errors,
        [format!(
            "{}: the package has no add-on content (it holds the game)",
            crate::package::io::abspath(&game_only).display()
        )]
    );
    // sync to nothing removes it all
    said.borrow_mut().clear();
    dlc::sync(&[], &datadir, &say, Some(&keys)).unwrap();
    assert_eq!(
        said.borrow()[0],
        format!("DLC: removed {:016x}", GAME + 0x1001)
    );
    assert!(dlc::read_manifest(&datadir).is_empty() && !datadir.join("dlc").exists());
}

/// An NSZ: the add-on's data NCA as an .ncz (header kept, section data stored decrypted behind zstd), solid or in
/// blocks; installed as `romfs.bin`.
#[test]
fn dlc_from_nsz_solid_and_block() {
    let (_t, keys) = setup("nsz");
    let fx = Fixture::new();
    let image = romfs_image(&[("contents.dat", &[5u8; 3000]), ("version.txt", b"20003")]);
    let mut sec = vec![0u8; 0x3800];
    sec.extend(&image);
    let plain_nca = build_nca(
        4,
        GAME + 0x1002,
        vec![(
            fs_header(false, 3, (0, 9), 0x3800, image.len() as u64),
            sec.clone(),
            None,
        )],
        fx.control_key,
        None,
    );
    let encrypted = build_nca(
        4,
        GAME + 0x1002,
        vec![(
            fs_header(false, 3, (0, 9), 0x3800, image.len() as u64),
            sec,
            plain_ctr(fx.control_key, (0, 9)),
        )],
        fx.control_key,
        None,
    );
    let datadir = _t.0.join("Game_data");
    std::fs::create_dir_all(datadir.join("exefs")).unwrap();
    let mut npdm = vec![0u8; 0x100];
    npdm[..4].copy_from_slice(b"META");
    npdm[0x70..0x74].copy_from_slice(&0x80u32.to_le_bytes());
    npdm[0x80..0x84].copy_from_slice(b"ACI0");
    npdm[0x90..0x98].copy_from_slice(&GAME.to_le_bytes());
    std::fs::write(datadir.join("exefs/main.npdm"), &npdm).unwrap();
    for block in [false, true] {
        let rest = &plain_nca[0x4000..];
        let mut ncz = encrypted[..0x4000].to_vec();
        ncz.extend(b"NCZSECTN");
        ncz.extend(1u64.to_le_bytes());
        ncz.extend([0u8; 0x40]);
        if block {
            let size = 1u64 << 14;
            let chunks: Vec<&[u8]> = rest.chunks(size as usize).collect();
            let packed: Vec<Vec<u8>> = chunks
                .iter()
                .map(|c| zstd::bulk::compress(c, 3).unwrap())
                .collect();
            ncz.extend(b"NCZBLOCK");
            ncz.extend([2u8, 1, 0, 14]);
            ncz.extend((chunks.len() as u32).to_le_bytes());
            ncz.extend((rest.len() as u64).to_le_bytes());
            for p in &packed {
                ncz.extend((p.len() as u32).to_le_bytes());
            }
            for p in packed {
                ncz.extend(p);
            }
        } else {
            ncz.extend(zstd::encode_all(rest, 3).unwrap());
        }
        let pkg = _t.0.join(format!("dlc{}.nsz", block as u8));
        write_pfs(
            &pkg,
            &[vec![
                (
                    format!("{}.cnmt.nca", "ce".repeat(16)),
                    meta_nca(GAME + 0x1002, 3, 0x82, &[("cf", 2)], fx.control_key),
                ),
                (format!("{}.ncz", "cf".repeat(16)), ncz),
            ]],
        );
        let said = std::cell::RefCell::new(vec![]);
        let (done, errors) = dlc::install(
            &[pkg.clone()],
            &datadir,
            &|m| said.borrow_mut().push(m.to_string()),
            Some(&keys),
            false,
        )
        .unwrap();
        assert_eq!((done.len(), errors.len()), (1, 0), "{:?}", said.borrow());
        assert!(
            said.borrow().last().unwrap().contains("(RomFS image, "),
            "{:?}",
            said.borrow()
        );
        let r = dlc::title_romfs(&datadir, GAME + 0x1002, None).unwrap();
        assert_eq!(r.read_file("version.txt").unwrap(), b"20003");
        assert_eq!(r.read_file("contents.dat").unwrap(), [5u8; 3000]);
        dlc::remove(&datadir, &format!("{:016x}", GAME + 0x1002), &|_| {}).unwrap();
    }
}

/// A bare data NCA, and an XCI holding the add-on, install like an NSP.
#[test]
fn dlc_from_bare_nca_and_xci() {
    let (_t, keys) = setup("dlcnca");
    let fx = Fixture::new();
    let datadir = _t.0.join("Game_data");
    std::fs::create_dir_all(datadir.join("exefs")).unwrap();
    let mut npdm = vec![0u8; 0x100];
    npdm[..4].copy_from_slice(b"META");
    npdm[0x70..0x74].copy_from_slice(&0x80u32.to_le_bytes());
    npdm[0x80..0x84].copy_from_slice(b"ACI0");
    npdm[0x90..0x98].copy_from_slice(&GAME.to_le_bytes());
    std::fs::write(datadir.join("exefs/main.npdm"), &npdm).unwrap();
    let addon = fx.addon();
    let bare = _t.0.join("addon.nca");
    std::fs::write(&bare, &addon[1].1).unwrap();
    let xci = _t.0.join("addon.xci");
    let refs: Vec<(&str, &[u8])> = addon
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    std::fs::write(&xci, build_xci(&refs, false)).unwrap();
    for (pkg, version) in [(&bare, 0), (&xci, 65536)] {
        let (done, errors) =
            dlc::install(&[pkg.clone()], &datadir, &|_| {}, Some(&keys), false).unwrap();
        assert_eq!((done, errors.len()), (vec![GAME + 0x1001], 0));
        assert_eq!(dlc::read_manifest(&datadir)[0].1, version);
        let r = dlc::title_romfs(&datadir, GAME + 0x1001, Some(&keys)).unwrap();
        assert_eq!(r.read_file("version.txt").unwrap(), b"10002");
        dlc::sync(&[], &datadir, &|_| {}, Some(&keys)).unwrap();
    }
}
