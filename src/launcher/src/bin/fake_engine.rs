//! Answers like nxrs-build (launcher contract v2) for the process and command-line tests, as the Qt
//! test binary's `main()` did: `contract`, `doctor` (one failing check), `update-info` (every package
//! but "other game" is update 1.1.1), `dlc sync` (every package but "refused" gets a manifest line),
//! and `build` (progress lines, then the build's files). `FAKE_LINES`, `FAKE_SLEEP_MS`, `FAKE_FAIL`
//! and `FAKE_CHILD` shape a build for the streaming and cancel tests.

use std::io::Write;
use std::path::Path;

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

/// The packages: what is not a flag or a flag's value.
fn packages(args: &[String], from: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = from;
    while let Some(a) = args.get(i) {
        if a == "--keys" || a == "--game" || a == "--datadir" {
            i += 2;
            continue;
        }
        out.push(a.clone());
        i += 1;
    }
    out
}

fn env_num(name: &str) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

fn doctor() -> i32 {
    println!(
        "ok   CPU: AVX2, FMA, BMI2 (the game code is built for x86-64-v3)\r\n     toolchain: clang + Visual Studio's libraries (MSVC ABI)\r"
    );
    println!(
        "ok   Vulkan loader (vulkan-1.dll)\r\nFAIL Vulkan device: Old GPU - Vulkan 1.2.0, driver 1.0 (the engine needs Vulkan 1.3: update the graphics driver)\r"
    );
    println!("ok   free disk for builds (C:\\nxrs): 120.5 GB\r");
    1
}

fn update_info(args: &[String]) -> i32 {
    let mut failed = 0;
    for p in packages(args, 1) {
        if p.contains("other game") {
            println!("UPDATE error: other game.nsp: this update is for another game");
            failed = 1;
        } else {
            println!("UPDATE\t{p}\t0100aaaa0000c000\t262144\t1.1.1");
        }
    }
    failed
}

fn dlc_sync(args: &[String]) -> i32 {
    let Some(data) = arg_after(args, "--datadir") else {
        return 2;
    };
    let mut manifest = String::new();
    let mut failed = 0;
    for p in packages(args, 2) {
        if p.contains("refused") {
            println!("DLC error: refused.nsp: the ticket is bound to one console");
            failed = 1;
        } else {
            manifest += &format!("0100152000023001 65536 data {p}\n");
            println!("DLC: installed 0100152000023001 v65536");
        }
    }
    if std::fs::write(Path::new(&data).join("dlc.manifest"), manifest).is_err() {
        return 3;
    }
    failed
}

fn build(args: &[String]) -> i32 {
    let (Some(dir), Some(name)) = (arg_after(args, "--outdir"), arg_after(args, "-o")) else {
        return 2;
    };
    if std::env::var("FAKE_CHILD").is_ok()
        && let Ok(me) = std::env::current_exe()
    {
        let _ = std::process::Command::new(me).arg("sleep").spawn();
    }
    println!("nxrs - use your own copy of the game");
    let lines = env_num("FAKE_LINES").max(3);
    let sleep = env_num("FAKE_SLEEP_MS");
    for i in 0..lines {
        println!("@progress\tcompile\t{}\t{lines}", i + 1);
        println!("compile unit {i}");
        let _ = std::io::stdout().flush();
        if sleep > 0 {
            std::thread::sleep(std::time::Duration::from_millis(sleep));
        }
    }
    if std::env::var("FAKE_FAIL").is_ok() {
        println!("@result\terror\tfailed on purpose");
        return 1;
    }
    let root = Path::new(&dir);
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.clone()
    };
    let module = if cfg!(windows) {
        format!("{name}_code.dll")
    } else {
        format!("{name}_code.so")
    };
    let made = std::fs::create_dir_all(root.join(format!("{name}_data"))).is_ok()
        && std::fs::create_dir_all(root.join(format!("{name}_build/gen"))).is_ok()
        && std::fs::write(root.join(&exe), b"fake build").is_ok()
        && std::fs::write(root.join(&module), b"fake module").is_ok()
        && std::fs::write(root.join(format!("{name}_build/gen/nxrs.gen.key")), b"k").is_ok()
        && std::fs::write(
            root.join(format!("{name}.ini")),
            b"[Graphics]\nrenderer=vulkan\n",
        )
        .is_ok();
    println!("@result\tok\t{exe}");
    if made { 0 } else { 1 }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match args.first().map(String::as_str) {
        Some("contract") => {
            println!("nxrs-contract 2");
            0
        }
        Some("doctor") => doctor(),
        Some("update-info") => update_info(&args),
        Some("dlc") if args.get(1).is_some_and(|a| a == "sync") => dlc_sync(&args),
        Some("build") => build(&args),
        Some("sleep") => {
            std::thread::sleep(std::time::Duration::from_secs(60));
            0
        }
        _ => 2,
    };
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}
