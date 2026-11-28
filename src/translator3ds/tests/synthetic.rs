//! the wave's fixtures: a synthetic .cxi (the donor rom.rs test shapes' format facts) translated end to
//! end, its C asserted by name, table and svc site, and compiled when `NXRS_3DS_CC_TESTS` opts the
//! compiler lanes in. No real title is opened; every byte here is made for the test.

use std::path::PathBuf;
use translator3ds::discover::Source;
use translator3ds::gen::{Generated, GenOptions3ds};
use translator3ds::rom::{self, Title};
use translator3ds::{discover, gen, image, overrides, Mods};

/// title id for test titles, no real game has it
const PROGRAM_ID: u64 = 0x0004_0000_0FF3_DE00;
/// the synthetic text's base, the donor fixtures'
const BASE: u32 = 0x0010_0000;
/// NCSD and NCCH offsets count in these
const MEDIA_UNIT: u64 = 0x200;

fn put(out: &mut [u8], at: usize, bytes: &[u8]) {
    out[at..at + bytes.len()].copy_from_slice(bytes);
}

fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|word| word.to_le_bytes()).collect()
}

/// a decrypted .cxi: NCCH header (never encrypted) at 0x100, ExeFS at 0x600 w/ .code as its only file,
/// the code all text at 0x00100000, and a RomFS holding files at their paths when there are any
fn cxi(code: &[u8], files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = vec![0; 0x800];
    put(&mut out, 0x100, b"NCCH");
    put(&mut out, 0x118, &PROGRAM_ID.to_le_bytes());
    out[0x18F] = 0x04;
    put(&mut out, 0x1A0, &3u32.to_le_bytes());
    put(&mut out, 0x200, b"test");
    let text = [BASE, code.len().div_ceil(0x1000) as u32, code.len() as u32];
    put(&mut out, 0x210, &text.map(u32::to_le_bytes).concat());
    put(&mut out, 0x600, b".code");
    put(&mut out, 0x60C, &(code.len() as u32).to_le_bytes());
    out.extend_from_slice(code);
    if !files.is_empty() {
        let (at, romfs) = (out.len().next_multiple_of(MEDIA_UNIT as usize), romfs(files));
        put(&mut out, 0x1B0, &((at as u64 / MEDIA_UNIT) as u32).to_le_bytes());
        put(&mut out, 0x1B4, &((romfs.len() as u64).div_ceil(MEDIA_UNIT) as u32).to_le_bytes());
        out.resize(at, 0);
        out.extend(romfs);
    }
    out
}

/// a RomFS w/o hashes, IVFC header then level 3, files at their paths in whatever folders those need
/// (the donor rom.rs fixture's tables; level 3 blocks of 16 bytes, no master hash)
fn romfs(files: &[(&str, &[u8])]) -> Vec<u8> {
    const NONE: u32 = 0xFFFF_FFFF;
    let mut dirs: Vec<(String, usize)> = vec![(String::new(), 0)];
    let mut leaves = Vec::new();
    for &(path, bytes) in files {
        let (folder, name) = path.rsplit_once('/').unwrap_or(("", path));
        let (mut parent, mut at) = (0, String::new());
        for part in folder.split('/').filter(|part| !part.is_empty()) {
            at = if at.is_empty() { part.to_owned() } else { format!("{at}/{part}") };
            parent = match dirs.iter().position(|(known, _)| *known == at) {
                Some(index) => index,
                None => {
                    dirs.push((at.clone(), parent));
                    dirs.len() - 1
                }
            };
        }
        leaves.push((parent, name, bytes));
    }
    let utf16 = |name: &str| -> Vec<u8> { name.encode_utf16().flat_map(u16::to_le_bytes).collect() };
    let dir_names: Vec<Vec<u8>> = dirs.iter().map(|(path, _)| utf16(path.rsplit('/').next().unwrap_or(""))).collect();
    let file_names: Vec<Vec<u8>> = leaves.iter().map(|(_, name, _)| utf16(name)).collect();
    let offsets = |names: &[Vec<u8>], header: usize| -> Vec<u32> {
        let mut end = 0;
        names
            .iter()
            .map(|name| {
                let at = end;
                end += header + name.len().next_multiple_of(4);
                at as u32
            })
            .collect()
    };
    let (dir_at, file_at) = (offsets(&dir_names, 0x18), offsets(&file_names, 0x20));
    let first = |found: Option<usize>, at: &[u32]| found.map_or(NONE, |index| at[index]);

    let mut dir_table = Vec::new();
    for (index, &(_, parent)) in dirs.iter().enumerate() {
        let next = if index == 0 { None } else { (index + 1..dirs.len()).find(|&other| dirs[other].1 == parent) };
        let child = (1..dirs.len()).find(|&other| dirs[other].1 == index);
        let file = (0..leaves.len()).find(|&other| leaves[other].0 == index);
        let name = &dir_names[index];
        let table = [dir_at[parent], first(next, &dir_at), first(child, &dir_at), first(file, &file_at), NONE, name.len() as u32];
        dir_table.extend(table.iter().flat_map(|word| word.to_le_bytes()));
        dir_table.extend_from_slice(name);
        dir_table.resize(dir_table.len().next_multiple_of(4), 0);
    }
    let (mut file_table, mut data) = (Vec::new(), Vec::new());
    for (index, &(parent, _, bytes)) in leaves.iter().enumerate() {
        data.resize(data.len().next_multiple_of(16), 0);
        let next = (index + 1..leaves.len()).find(|&other| leaves[other].0 == parent);
        let name = &file_names[index];
        file_table.extend([dir_at[parent], first(next, &file_at)].iter().flat_map(|word| word.to_le_bytes()));
        file_table.extend([data.len() as u64, bytes.len() as u64].iter().flat_map(|word| word.to_le_bytes()));
        file_table.extend([NONE, name.len() as u32].iter().flat_map(|word| word.to_le_bytes()));
        file_table.extend_from_slice(name);
        file_table.resize(file_table.len().next_multiple_of(4), 0);
        data.extend_from_slice(bytes);
    }

    let mut out = vec![0; 0x60];
    put(&mut out, 0, b"IVFC");
    out[0x4C] = 4;
    let (dirs_at, files_at) = (0x28, 0x28 + dir_table.len());
    let data_at = (files_at + file_table.len()).next_multiple_of(16);
    let header = [0x28usize, dirs_at, 0, dirs_at, dir_table.len(), files_at, 0, files_at, file_table.len(), data_at];
    out.extend(header.iter().flat_map(|&word| (word as u32).to_le_bytes()));
    out.extend(dir_table);
    out.extend(file_table);
    out.resize(0x60 + data_at, 0);
    out.extend(data);
    out
}

/// a CRO module called name, code the words at 0x200, exporting where that starts (the donor cro.rs fixture)
fn module(name: &str, code: &[u32]) -> Vec<u8> {
    let mut bytes = vec![0; 0x200 + code.len() * 4];
    let mut put_word = |at: usize, value: u32| bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
    put_word(0x80, u32::from_le_bytes(*b"CRO0"));
    put_word(0xC0, 0x180);
    // one segment (the code) and one export by index, its start
    put_word(0xC8, 0x138);
    put_word(0xCC, 1);
    put_word(0x138, 0x200);
    put_word(0x13C, code.len() as u32 * 4);
    put_word(0xD8, 0x150);
    put_word(0xDC, 1);
    for (i, &word) in code.iter().enumerate() {
        put_word(0x200 + i * 4, word);
    }
    bytes[0x180..0x180 + name.len()].copy_from_slice(name.as_bytes());
    bytes
}

/// the entry answers the tick, answers a plain svc, calls a function, and comes back
fn entry_code() -> Vec<u8> {
    words(&[
        0xEF00_0028, // svc 0x28, GetSystemTick
        0xEF00_0032, // svc 0x32, one the host always answers
        0xEB00_0000, // bl 0x00100010
        0xE12F_FF1E, // bx lr
        0xE3A0_0001, // mov r0, 1 (the called function)
        0xE12F_FF1E, // bx lr
    ])
}

fn write_fixture(name: &str, code: &[u8], files: &[(&str, &[u8])]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("translator3ds-{name}-{}.cxi", std::process::id()));
    std::fs::write(&path, cxi(code, files)).unwrap();
    path
}

fn generate_into(dir: &std::path::Path, title: &Title, hints: &[u32]) -> Generated {
    gen::generate(
        title,
        Mods::default(),
        GenOptions3ds { dir, overrides: None, hints: hints.to_vec() },
        &|_| {},
    )
    .unwrap()
}

/// the fixture translates: the C names its functions, carries an Entry table and the svc sites
#[test]
fn a_synthetic_cxi_translates_to_c() {
    let path = write_fixture("translate", &entry_code(), &[]);
    let opened = translator3ds::title::open(&path).unwrap();
    assert_eq!(opened.kind, translator3ds::title::DsKind::Cxi);
    assert_eq!(opened.title.program_id(), PROGRAM_ID);

    let dir = std::env::temp_dir().join(format!("translator3ds-gen-{}", std::process::id()));
    let generated = generate_into(&dir, &opened.title, &[]);
    assert_eq!(generated.dir, dir);
    assert_eq!(generated.files, 4, "recomp.h, functions.h, code000.c, entries.c");
    let code = std::fs::read_to_string(dir.join("code000.c")).unwrap();
    // the functions, by name
    assert!(code.contains("uint32_t f_00100000(Context *ctx)"), "{code}");
    assert!(code.contains("uint32_t f_00100010(Context *ctx)"), "{code}");
    // the call between them, w/ its return check
    assert!(code.contains("CALL_RETURNING(f_00100010, 0x0010000Cu);"), "{code}");
    // the svc sites: the tick answered in place, the plain one handed to the host
    assert!(code.contains("SVC_TICK(0x00100004u, 0u);"), "{code}");
    assert!(code.contains("SVC(0x00100008u, 0x32u);"), "{code}");
    // the Entry table, sorted, one place execution can come back to each
    let tables = std::fs::read_to_string(dir.join("entries.c")).unwrap();
    assert!(tables.contains("RECOMP_EXPORT const Entry recomp_entries[]"), "{tables}");
    assert!(tables.contains("RECOMP_EXPORT const uint32_t recomp_entry_count = "), "{tables}");
    assert!(tables.contains("recomp_starts[2] = {0x00100000u, 0x00100010u};"), "{tables}");
    assert!(tables.contains("RECOMP_EXPORT const uint32_t recomp_abi = RECOMP_ABI;"), "{tables}");
    // the header rides the generated files, verbatim
    let header = std::fs::read_to_string(dir.join("recomp.h")).unwrap();
    assert_eq!(header, translator3ds::abi::HEADER);
    assert!(header.contains("#define RECOMP_ABI 8"));
    drop(opened);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(&path).unwrap();
}

/// the generated C compiles to objects and archives into a static lib, when the compile lanes opt in
#[test]
fn the_generated_c_compiles() {
    if std::env::var_os("NXRS_3DS_CC_TESTS").is_none() {
        return;
    }
    let Some(compiler) = translator3ds::compile::Compiler::find() else {
        eprintln!("skipping: there is no C compiler");
        return;
    };
    // the archiver rides the compiler's own toolchain folder (clang's llvm-ar); on the paths unix's ar answers
    if cfg!(windows) {
        let llvm_ar = compiler.program.parent().unwrap().join("llvm-ar.exe");
        assert!(llvm_ar.is_file(), "llvm-ar beside the compiler");
        std::env::set_var("AR", llvm_ar);
    }
    let path = write_fixture("compile", &entry_code(), &[]);
    let opened = translator3ds::title::open(&path).unwrap();
    let dir = std::env::temp_dir().join(format!("translator3ds-compile-{}", std::process::id()));
    let generated = generate_into(&dir, &opened.title, &[]);
    drop(opened);
    let objects = translator3ds::compile::objects(&compiler, &dir, &generated.sources, &|_, _| true, false).unwrap();
    let library = dir.join("translator3ds_w0.a");
    translator3ds::compile::archive(&objects, &library).unwrap();
    assert!(library.metadata().unwrap().len() > 0);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(&path).unwrap();
}

/// a module in the RomFS becomes its own program beside the exe, its tables prefixed
/// and behind a base the host writes when the title loads it
#[test]
fn a_module_in_the_romfs_is_its_own_program() {
    let (crs, battle) = (module("|static|", &[]), module("Battle", &[0xE3A0_0002, 0xE12F_FF1E]));
    let files: [(&str, &[u8]); 2] =
        [("static.crs", &crs[..]), ("cro/Battle.cro", &battle[..])];
    let path = write_fixture("module", &words(&[0xE12F_FF1E]), &files);
    let title = Title::load(&path).unwrap();
    let programs = translator3ds::programs(&title, Mods::default()).unwrap();
    assert_eq!(programs.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>(), ["executable", "Battle"]);

    let dir = std::env::temp_dir().join(format!("translator3ds-module-{}", std::process::id()));
    let generated = generate_into(&dir, &title, &[]);
    let tables = std::fs::read_to_string(dir.join("entries.c")).unwrap();
    assert!(tables.contains("uint32_t m001_base;"), "{tables}");
    assert!(tables.contains("static const Entry m001_entries[]"), "{tables}");
    assert!(tables.contains("{\"Battle\", &m001_base,"), "{tables}");
    let code = std::fs::read_to_string(dir.join("code000.c")).unwrap();
    assert!(code.contains("#include \"m001_functions.h\""), "{code}");
    assert!(code.contains("m001_f_00000200"), "{code}");
    assert_eq!(generated.files, 5, "recomp.h, functions.h, m001_functions.h, code000.c, entries.c");
    drop(title);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(&path).unwrap();
}

/// code and modules a mod changes make C of the mod's bytes, so a host running the mod finds nothing stale
#[test]
fn generating_with_mods_makes_code_of_what_they_change() {
    let piece = |start: u32, end: u32, bytes: &[u8]| -> String {
        let mut hash = translator3ds::abi::CodeHash::default();
        hash.add(bytes);
        format!("{{0x{start:08X}u, 0x{end:08X}u, 0x{:016X}ull}},", hash.value())
    };
    let own = words(&[0xE3A0_0000, 0xE12F_FF1E]);
    let battle = module("Battle", &[0xE3A0_0000, 0xE12F_FF1E]);
    let files = [("cro/Battle.cro", &battle[..])];
    let path = write_fixture("mods", &own, &files);
    let title = Title::load(&path).unwrap();
    let dir = std::env::temp_dir().join(format!("translator3ds-mods-{}", std::process::id()));
    let (code, battle_modded) = (words(&[0xE3A0_0001, 0xE12F_FF1E]), module("Battle", &[0xE3A0_0001, 0xE12F_FF1E]));
    let romfs = |asked: &str| (asked == "cro/Battle.cro").then(|| battle_modded.clone());
    let tables = |mods: Mods| {
        gen::generate(&title, mods, GenOptions3ds { dir: &dir, overrides: None, hints: Vec::new() }, &|_| {}).unwrap();
        std::fs::read_to_string(dir.join("entries.c")).unwrap()
    };
    let modded_tables = tables(Mods { code: Some(&code), romfs: Some(&romfs), ..Mods::default() });
    // the exe's func and the module's, a piece each, hashed from the mod's bytes
    assert!(modded_tables.contains(&piece(BASE, BASE + 8, &code)), "{modded_tables}");
    assert!(modded_tables.contains(&piece(0x200, 0x208, &code)), "{modded_tables}");
    let plain = tables(Mods::default());
    assert!(plain.contains(&piece(BASE, BASE + 8, &own)), "{plain}");
    assert!(plain.contains(&piece(0x200, 0x208, &own)), "{plain}");
    drop(title);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(&path).unwrap();
}

/// a hint the analysis missed starts a function: no hint, no function there; with one, the C names it
#[test]
fn a_hint_starts_a_function_the_analysis_missed() {
    // the entry returns; the gap starts with a conditional branch, so the gap scan does not start a func
    let mut code_words = vec![0xE12F_FF1E, 0x1A00_0000, 0xE1A0_0000, 0xE1A0_0000, 0xE12F_FF1E];
    code_words.resize(64, 0);
    let path = write_fixture("hints", &words(&code_words), &[]);
    let title = Title::load(&path).unwrap();
    let dir = std::env::temp_dir().join(format!("translator3ds-hints-{}", std::process::id()));
    let plain = generate_into(&dir, &title, &[]);
    let without = std::fs::read_to_string(dir.join("code000.c")).unwrap();
    assert!(!without.contains("f_00100004"), "{without}");
    let hinted = generate_into(&dir, &title, &[BASE + 4]);
    assert_eq!(plain.files, hinted.files);
    let code = std::fs::read_to_string(dir.join("code000.c")).unwrap();
    assert!(code.contains("uint32_t f_00100004(Context *ctx)"), "{code}");
    let tables = std::fs::read_to_string(dir.join("entries.c")).unwrap();
    assert!(tables.contains("0x00100004u"), "{tables}");
    drop(title);
    std::fs::remove_dir_all(&dir).unwrap();
    std::fs::remove_file(&path).unwrap();
}

/// the vendored pub front half answers as upstream on fixture shapes
#[test]
fn the_vendored_front_half_answers_as_upstream() {
    // image: segments at the addresses the exheader gives
    let exheader = rom::ExHeader::read(&{
        let mut ex = vec![0u8; 0x400];
        let text = [BASE, 1u32, 12];
        put(&mut ex, 0x10, &text.map(u32::to_le_bytes).concat());
        ex
    });
    let image = image::Image::from_code(&exheader, &words(&[0xEB00_0000, 0xE12F_FF1E, 0x4770_4770]));
    assert_eq!(image.entry, BASE);
    assert_eq!(image.text.base, BASE);
    // overrides: the marks parse
    let parsed = overrides::parse("RECOMP_OVERRIDE(0x0012A4F1) { RETURN_TO(ctx->r[14]); }").unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].address, 0x0012_A4F1);
    // discovery: a pointer seed starts a thumb func where nothing surer leads (odd value = Thumb)
    let mut program = image::Image::from_code(&exheader, &words(&[0xE12F_FF1E, 0x0000_0000, 0x4770_4770])).into_program(&[], &[]);
    program.seeds.push((BASE + 9, Source::Pointer));
    let analysis = discover::analyze(&program);
    assert!(analysis.functions.contains_key(&(BASE + 8)));
    assert_eq!(analysis.functions[&(BASE + 8)].mode, discover::Mode::Thumb);
    assert_eq!(analysis.svc_sites, 0);
}
