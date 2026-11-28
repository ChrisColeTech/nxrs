//! the headless boot harness: the staged dump → translate → compile (the donor flag set) →
//! load through the mirror → map the segments → boot the entry + first threads under a budget,
//! with a svc/gap log, the coverage numbers and the miss files under the output folder.
//!
//! ```powershell
//! cargo run -p runtime3ds --bin boot3ds -- --dump <game.3ds> --out build\DS\W2\sonic
//! ```

use std::path::PathBuf;
use std::time::Instant;
use translator3ds::gen::{GenOptions3ds, Generated};
use translator3ds::Mods;

const MEG: f64 = 1048576.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut dump, mut out, mut dll, mut slice, mut seconds) = (None, None, None, 200_000i32, 90u64);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dump" => dump = args.next(),
            "--out" => out = args.next(),
            "--dll" => dll = args.next(),
            "--slice" => slice = args.next().and_then(|v| v.parse().ok()).unwrap_or(slice),
            "--seconds" => seconds = args.next().and_then(|v| v.parse().ok()).unwrap_or(seconds),
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(dump) = dump.map(PathBuf::from) else {
        eprintln!("usage: boot3ds --dump <game.3ds|.cia> --out <dir> [--dll code.dll] [--slice n] [--seconds n]");
        std::process::exit(2);
    };
    let out = out.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("build/DS/W2/boot"));
    std::fs::create_dir_all(&out).expect("the output folder");
    if let Err(error) = run(&dump, &out, dll, slice, seconds) {
        eprintln!("boot3ds: {error}");
        std::process::exit(1);
    }
}

fn run(dump: &std::path::Path, out: &std::path::Path, dll: Option<String>, slice: i32, seconds: u64) -> Result<(), String> {
    let say = |line: String| {
        println!("{line}");
        use std::io::Write;
        let _ = std::io::stdout().flush();
    };
    say(format!("boot3ds: {}", dump.display()));
    let opened = translator3ds::title::open(dump).map_err(|e| e.to_string())?;
    let program_id = opened.title.program_id();
    say(format!("program id {program_id:016X}"));
    let library_path = if let Some(path) = dll {
        let path = PathBuf::from(path);
        say(format!("using {}", path.display()));
        path
    } else {
        // translate: the C under <out>\gen, hints from the last run's misses when there are any
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
            generated.bytes as f64 / MEG,
            began.elapsed().as_secs_f64()
        ));
        let compiler = translator3ds::compile::check(None)?;
        say(format!("compiler: {}", compiler.describe()));
        let library_path = out.join(format!("{program_id:016X}.dll"));
        let began = Instant::now();
        translator3ds::compile::compile(&compiler, &gen_dir, &generated.sources, &library_path, &|done, total| {
            if done % 20 == 0 || done == total {
                say(format!("compiling {done}/{total}"));
            }
            true
        }, false)
        .map_err(|e| e.to_string())?;
        say(format!(
            "compiled {} ({:.1}s), {}",
            library_path.display(),
            began.elapsed().as_secs_f64(),
            content_hash(&library_path)
        ));
        library_path
    };
    // the staged image: the exe's segments at the addresses its exheader names
    let image = translator3ds::image::Image::from_title(&opened.title).map_err(|e| e.to_string())?;
    let code_bytes = opened.title.code().map_err(|e| e.to_string())?;
    let bss = opened.title.exheader.bss_size;
    // load through the mirror
    let mut library = runtime3ds::dispatch::Library::open(&library_path)?;
    say(format!(
        "code: {} (abi {}, generation {})",
        library.describe(),
        library.abi,
        library.generation
    ));
    let memory = std::sync::Arc::new(runtime3ds::mem::Memory::new().map_err(|e| e.to_string())?);
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
    // the map the wave doc pins: heap window, LINEAR window, stack, VRAM (writes watched), MMIO,
    // DSP, the config pages
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
    // the kernel configuration page: FCRAM size, APPMEMTYPE 0, the three region sizes (3dbrew
    // FCRAM table, Memory_layout); the page is read-only to the guest, the loader pokes it
    let poke = |address: u32, value: u32| memory.load_bytes(address, &value.to_le_bytes());
    poke(0x1FF8_0040, 0x0400_0000);
    poke(0x1FF8_0048, 0);
    poke(0x1FF8_0058, 0x0400_0000);
    poke(0x1FF8_005C, 0x02C0_0000);
    poke(0x1FF8_0060, 0x0140_0000);
    // the origins check for the exe: the funcs stale-mark if the memory differs from what was
    // made (the modules' check waits for W3's ldr::ro to place them)
    let stale = library.check(0, 0, |address, out| {
        let offset = (address - image.text.base) as usize;
        let span = code_bytes.len();
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = code_bytes.get(offset + i).copied().unwrap_or(0);
        }
        let _ = span;
    });
    say(format!("origins checked, {stale} funcs stale"));
    // boot
    let entry = image.entry;
    let options = runtime3ds::BootOptions {
        slice,
        time_cap: std::time::Duration::from_secs(seconds),
        ..Default::default()
    };
    let process = runtime3ds::Process3ds::new(library, memory.clone(), options);
    say(format!("booting at {entry:#x} (main stack 0x10000000, 0x4000)"));
    let began = Instant::now();
    let report = runtime3ds::start(&process, program_id, entry, 0x1000_0000, 0x4000)?;
    let seconds_ran = began.elapsed().as_secs_f64();
    // the reports and the miss files
    let mut log = format!("boot3ds {}\n", dump.display());
    log.push_str(&format!("program id {program_id:016X}\ncode {}\n", report.describe));
    log.push_str(&format!("dll sha {}\n", content_hash(&library_path)));
    for line in &report.log {
        log.push_str(line);
        log.push('\n');
    }
    let total = report.recompiled + report.interpreted;
    log.push_str(&format!(
        "\nstop: {:?}\nticks {} ({:.1}M, {:.1}s host)\nrecompiled {} interpreted {} ({:.2}% interpreted)\n",
        report.stop,
        report.ticks,
        report.ticks as f64 / MEG,
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
    for (line, count) in &report.gaps {
        log.push_str(&format!("gap [{count}x] {line}\n"));
    }
    let miss_sites = process.recorder.addresses();
    for (address, count, why) in miss_sites.iter().take(60) {
        log.push_str(&format!("miss {address:#x} [{count}x] {why}\n"));
    }
    let mut starts = report.interp_starts.clone();
    starts.sort_by_key(|&(_, count)| std::cmp::Reverse(count));
    for (address, count) in starts.iter().take(30) {
        log.push_str(&format!("interpreted from {address:#x} [{count}x]\n"));
    }
    std::fs::write(out.join("boot.log"), &log).map_err(|e| e.to_string())?;
    let hints_written = process.recorder.write_hints(&out.join("discovered.tsv")).map_err(|e| e.to_string())?;
    let hot_written = process.recorder.write_hotset(&out.join("hotset.txt")).map_err(|e| e.to_string())?;
    say(format!("stop: {:?}", report.stop));
    say(format!(
        "ticks {}M, recompiled {} interpreted {} ({}% interpreted), svcs {}, gaps {}, misses {}",
        report.ticks / 1_000_000,
        report.recompiled,
        report.interpreted,
        if total > 0 { report.interpreted * 100 / total } else { 100 },
        report.svcs,
        report.gaps.len(),
        report.misses
    ));
    say(format!(
        "artifacts: {}, discovered.tsv ({hints_written}), hotset.txt ({hot_written})",
        out.join("boot.log").display()
    ));
    Ok(())
}

/// the DLL's content hash for the log (a 64-bit FNV1a over the bytes, named what it is)
fn content_hash(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).unwrap_or_default();
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for byte in &bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01B3);
    }
    format!("fnv1a:{hash:016X} ({} bytes)", bytes.len())
}
