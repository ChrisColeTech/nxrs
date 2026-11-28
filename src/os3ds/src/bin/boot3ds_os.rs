//! the W3 headless harness: open the staged title, load/reuse the generated DLL, map the exe's
//! segments and the wave doc's regions, install os3ds's services, run the scripted input, and write
//! the boot log, the gap summary and the input script/consumed log under the output folder.

use os3ds::{parse_input, Host};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use translator3ds::gen::{Generated, GenOptions3ds};
use translator3ds::Mods;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut dump, mut out, mut dll, mut romfs, mut save, mut input) = (None, None, None, None, None, None);
    let (mut slice, mut seconds) = (200_000i32, 90u64);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dump" => dump = args.next(),
            "--out" => out = args.next(),
            "--dll" => dll = args.next(),
            "--romfs" => romfs = args.next(),
            "--save" => save = args.next(),
            "--input" => input = args.next(),
            "--slice" => slice = args.next().and_then(|v| v.parse().ok()).unwrap_or(slice),
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()).unwrap_or(seconds),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(dump) = dump.map(PathBuf::from) else {
        eprintln!("usage: boot3ds_os --dump <game.3ds> --out <dir> [--dll code.dll] [--romfs dir] [--save dir] [--input script] [--slice n] [--seconds n]");
        std::process::exit(2);
    };
    let out = out.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("build/DS/W3/boot"));
    std::fs::create_dir_all(&out).expect("the output folder");
    if let Err(error) = run(&dump, &out, dll.map(PathBuf::from), romfs.map(PathBuf::from), save.map(PathBuf::from), input.map(PathBuf::from), slice, seconds) {
        eprintln!("boot3ds_os: {error}");
        std::process::exit(1);
    }
}

fn run(
    dump: &Path,
    out: &Path,
    dll: Option<PathBuf>,
    romfs: Option<PathBuf>,
    save: Option<PathBuf>,
    input: Option<PathBuf>,
    slice: i32,
    seconds: u64,
) -> Result<(), String> {
    let say = |line: String| {
        println!("{line}");
        use std::io::Write;
        let _ = std::io::stdout().flush();
    };
    say(format!("boot3ds_os: {}", dump.display()));
    let opened = translator3ds::title::open(dump).map_err(|e| e.to_string())?;
    let program_id = opened.title.program_id();
    say(format!("program id {program_id:016X}"));
    let library_path = match dll {
        Some(path) => {
            say(format!("using {}", path.display()));
            path
        }
        None => {
            let hints_path = out.join("discovered.tsv");
            let hints = std::fs::read_to_string(&hints_path)
                .map(|text| translator3ds::hints::read_hints(&text))
                .unwrap_or_default();
            let gen_dir = out.join("gen");
            let began = Instant::now();
            let generated: Generated = translator3ds::gen::generate(
                &opened.title,
                Mods::default(),
                GenOptions3ds { dir: &gen_dir, overrides: None, hints },
                &|_| {},
            )
            .map_err(|e| e.to_string())?;
            say(format!(
                "C generated: {} files, {:.1} MB ({:.1}s)",
                generated.files,
                generated.bytes as f64 / 1048576.0,
                began.elapsed().as_secs_f64()
            ));
            let compiler = translator3ds::compile::check(None)?;
            say(format!("compiler: {}", compiler.describe()));
            let library_path = out.join(format!("{program_id:016X}.dll"));
            translator3ds::compile::compile(&compiler, &gen_dir, &generated.sources, &library_path, &|done, total| {
                if done % 40 == 0 || done == total {
                    say(format!("compiling {done}/{total}"));
                }
                true
            }, false)
            .map_err(|e| e.to_string())?;
            library_path
        }
    };
    // the RomFS root is the staged folder (read-only); the save root is fresh under the output
    let romfs = romfs.unwrap_or_else(|| out.join("Sonic_data/romfs"));
    let save = save.unwrap_or_else(|| out.join("save"));
    std::fs::create_dir_all(&save).map_err(|e| e.to_string())?;
    say(format!("romfs root {} (read-only)", romfs.display()));
    say(format!("save root {}", save.display()));
    // the input script: the named file, or the wave doc's default
    let script_text = match &input {
        Some(path) => std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?,
        None => "0 neutral\n2500 start\n3500 neutral\n5000 a\n6000 neutral\n".to_owned(),
    };
    let steps = parse_input(&script_text)?;
    std::fs::write(out.join("input.txt"), &script_text).map_err(|e| e.to_string())?;
    say(format!("input script: {} steps", steps.len()));
    // the staged image
    let image = translator3ds::image::Image::from_title(&opened.title).map_err(|e| e.to_string())?;
    let code_bytes = opened.title.code().map_err(|e| e.to_string())?;
    let bss = opened.title.exheader.bss_size;
    let mut library = runtime3ds::dispatch::Library::open(&library_path)?;
    say(format!(
        "code: {} (abi {}, generation {})",
        library.describe(),
        library.abi,
        library.generation
    ));
    let memory = Arc::new(runtime3ds::mem::Memory::new().map_err(|e| e.to_string())?);
    let rounded = |bytes: u32| bytes.next_multiple_of(0x1000);
    for (base, bytes, perm) in [
        (image.text.base, &image.text.bytes, runtime3ds::mem::Perm::R),
        (image.rodata.base, &image.rodata.bytes, runtime3ds::mem::Perm::R),
    ] {
        memory
            .map(base, rounded(bytes.len() as u32).max(0x1000), perm, runtime3ds::mem::State::Code, runtime3ds::mem::Kind::Exe)
            .map_err(|e| format!("{base:#x}: {e}"))?;
        memory.load_bytes(base, bytes);
    }
    let data_span = rounded(image.data.bytes.len() as u32 + bss);
    memory
        .map(image.data.base, data_span, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Private, runtime3ds::mem::Kind::Exe)
        .map_err(|e| format!("{:#x}: {e}", image.data.base))?;
    memory.write_bytes(image.data.base, &image.data.bytes);
    for (base, size, perm, state, kind) in [
        (0x0800_0000, 0x0400_0000, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Private, runtime3ds::mem::Kind::Heap),
        (0x1400_0000, 0x0800_0000, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Continuous, runtime3ds::mem::Kind::Linear),
        (0x0FF0_0000, 0x0010_0000, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Private, runtime3ds::mem::Kind::Stack),
        (0x1F00_0000, 0x0060_0000, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Locked, runtime3ds::mem::Kind::Vram),
        (0x1EC0_0000, 0x0040_0000, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Io, runtime3ds::mem::Kind::Mmio),
        (0x1FF0_0000, 0x0008_0000, runtime3ds::mem::Perm::Rw, runtime3ds::mem::State::Locked, runtime3ds::mem::Kind::Dsp),
        (0x1FF8_0000, 0x0000_1000, runtime3ds::mem::Perm::R, runtime3ds::mem::State::Static, runtime3ds::mem::Kind::Config),
        (0x1FF8_1000, 0x0000_1000, runtime3ds::mem::Perm::R, runtime3ds::mem::State::Static, runtime3ds::mem::Kind::SharedConfig),
    ] {
        memory.map(base, size, perm, state, kind).map_err(|e| format!("{base:#x}: {e}"))?;
    }
    let poke = |address: u32, value: u32| memory.load_bytes(address, &value.to_le_bytes());
    poke(0x1FF8_0030, 0);
    poke(0x1FF8_0040, 0x0400_0000);
    poke(0x1FF8_0044, 0x02C0_0000);
    poke(0x1FF8_0048, 0x0140_0000);
    let stale = library.check(0, 0, |address, out| {
        let offset = (address - image.text.base) as usize;
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = code_bytes.get(offset + i).copied().unwrap_or(0);
        }
    });
    say(format!("origins checked, {stale} funcs stale"));
    let host = Host::new_with(&romfs, &save, out.join("sdmc"), steps.clone());
    match romfs_image_range(dump) {
        Some((base, size)) => {
            host.set_romfs_image(dump, base, size);
            say(format!("romfs image: dump + {base:#x}, {size:#x} bytes"));
        }
        None => say("romfs image: the dump's IVFC level-3 range was not found (directory fallback)".to_owned()),
    }
    let options = runtime3ds::BootOptions {
        slice,
        time_cap: std::time::Duration::from_secs(seconds),
        ..Default::default()
    };
    let process = runtime3ds::Process3ds::new(library, memory.clone(), options);
    say(format!("booting at {:#x} (main stack 0x10000000, 0x4000)", image.entry));
    let began = Instant::now();
    let report = os3ds::nxos3ds_main(host.clone(), &process, program_id, image.entry, 0x1000_0000, 0x4000)?;
    let seconds_ran = began.elapsed().as_secs_f64();
    std::thread::sleep(std::time::Duration::from_millis(40));
    // the log
    let mut log = format!("boot3ds_os {}\n", dump.display());
    log.push_str(&format!("program id {program_id:016X}\ncode {}\n", report.describe));
    log.push_str(&format!("dll sha {}\n", content_hash(&library_path)));
    log.push_str(&format!("romfs {}\nsave {}\n", romfs.display(), save.display()));
    log.push_str("\n-- os3ds --\n");
    for line in host.log() {
        log.push_str(&line);
        log.push('\n');
    }
    log.push_str("\n-- runtime3ds --\n");
    for line in &report.log {
        log.push_str(line);
        log.push('\n');
    }
    let total = report.recompiled + report.interpreted;
    log.push_str(&format!(
        "\nstop: {:?}\nticks {} ({:.1}M, {:.1}s host)\nrecompiled {} interpreted {} ({:.2}% interpreted)\n",
        report.stop,
        report.ticks,
        report.ticks as f64 / 1048576.0,
        seconds_ran,
        report.recompiled,
        report.interpreted,
        if total > 0 { report.interpreted as f64 * 100.0 / total as f64 } else { 100.0 }
    ));
    log.push_str(&format!(
        "svcs {} answered in place by the code {}, gaps {}, cache ops {}\n",
        report.svcs, report.ticks_answered_in_place, report.gaps.len(), report.cache_ops
    ));
    log.push_str(&format!(
        "misses {} at {} sites\nfaults {} (last {:#x}) mmio reads {} writes {} watched writes {}\n",
        report.misses, report.miss_sites, report.faults, report.last_fault, report.mmio_reads, report.mmio_writes, report.watched_writes
    ));
    log.push_str(&format!("threads that ran: {}\n", report.threads_ran));
    for (number, result, count) in &report.svcs_seen {
        log.push_str(&format!(
            "svc [{count}x] {number:#04x} {} -> {result:#010x}\n",
            runtime3ds::svc::svc_name(*number)
        ));
    }
    log.push_str("\n-- gap summary (runtime3ds kernel + os3ds services) --\n");
    for (line, count) in &report.gaps {
        log.push_str(&format!("gap [{count}x] {line}\n"));
    }
    for (line, count) in host.gaps() {
        log.push_str(&format!("gap [{count}x] {line}\n"));
    }
    log.push_str("\n-- input --\n");
    let (applied, serves, updates) = {
        let input = host.input.lock().unwrap();
        (input.applied.clone(), input.serves, input.updates)
    };
    log.push_str(&format!("hid shared-memory serves {serves}, pad updates {updates}\n"));
    log.push_str(&format!(
        "gsp vblank pairs pushed {}, frames {}\n",
        host.vblanks.load(std::sync::atomic::Ordering::Relaxed),
        host.frames.load(std::sync::atomic::Ordering::Relaxed)
    ));
    for index in 0..8 {
        if let Some((pc, budget, thumb)) = process.thread_pc(index) {
            log.push_str(&format!("thread {index} parked at {pc:#x} (thumb {thumb}, budget {budget})\n"));
        }
    }
    for line in &applied {
        log.push_str(&format!("input applied {line}\n"));
    }
    std::fs::write(out.join("boot.log"), &log).map_err(|e| e.to_string())?;
    std::fs::write(out.join("gaps.txt"), {
        let mut text = String::new();
        for (line, count) in report.gaps.iter().chain(host.gaps().iter()) {
            text.push_str(&format!("[{count}x] {line}\n"));
        }
        text
    })
    .map_err(|e| e.to_string())?;
    std::fs::write(out.join("input-consumed.txt"), {
        let mut text = format!("script {} steps, serves {serves}, pad updates {updates}\n", steps.len());
        for line in &applied {
            text.push_str(line);
            text.push('\n');
        }
        text
    })
    .map_err(|e| e.to_string())?;
    if !report.interp_starts.is_empty() {
        let hints = process.recorder.write_hints(&out.join("discovered.tsv")).map_err(|e| e.to_string())?;
        let _ = process.recorder.write_hotset(&out.join("hotset.txt"));
        say(format!("hints written: {hints}"));
    }
    say(format!("stop: {:?}", report.stop));
    say(format!(
        "ticks {}M, svcs {}, runtime gaps {}, os3ds gaps {}, input steps {}",
        report.ticks / 1_000_000,
        report.svcs,
        report.gaps.len(),
        host.gaps().len(),
        applied.len()
    ));
    say(format!("artifacts: {}", out.join("boot.log").display()));
    Ok(())
}


/// the RomFS IVFC level-3 image: (offset in the dump, size). A cart's exe is partition 0; a bare
/// NCCH/CXI sits at 0. Mirrors 3dbrew `RomFS` (header 0x5C, level-3 logical offset/size at
/// 0x3C/0x44, block log2 at 0x4C) and translator3ds's `read_romfs`.
fn romfs_image_range(dump: &Path) -> Option<(u64, u64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(dump).ok()?;
    let mut head = [0u8; 0x200];
    file.read_exact(&mut head).ok()?;
    let word = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let ncch = if &head[0x100..0x104] == b"NCSD" {
        word(&head, 0x120) as u64 * 0x200
    } else if &head[0x100..0x104] == b"NCCH" {
        0
    } else {
        return None;
    };
    let mut ncch_header = [0u8; 0x200];
    file.seek(SeekFrom::Start(ncch)).ok()?;
    file.read_exact(&mut ncch_header).ok()?;
    if &ncch_header[0x100..0x104] != b"NCCH" {
        return None;
    }
    let romfs_offset = ncch + word(&ncch_header, 0x1B0) as u64 * 0x200;
    let mut ivfc = [0u8; 0x5C];
    file.seek(SeekFrom::Start(romfs_offset)).ok()?;
    file.read_exact(&mut ivfc).ok()?;
    if &ivfc[..4] != b"IVFC" {
        return None;
    }
    let u64_at = |at: usize| u64::from_le_bytes(ivfc[at..at + 8].try_into().unwrap());
    let master_hash = word(&ivfc, 0x08) as u64;
    let block = 1u64 << word(&ivfc, 0x4C).min(31);
    let level3 = romfs_offset + (0x60 + master_hash).next_multiple_of(block);
    let size = u64_at(0x44);
    (size > 0).then_some((level3, size))
}

/// the DLL's content hash for the log (a 64-bit FNV1a over the bytes)
fn content_hash(path: &Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in &bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01B3);
    }
    format!("fnv1a:{hash:016X} ({} bytes)", bytes.len())
}
