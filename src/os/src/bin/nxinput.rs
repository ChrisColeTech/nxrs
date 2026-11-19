//! nxinput: the controllers the engine sees, without a game (Turbo's `controller_device` / `test_controllers.py`).
//!
//!     nxinput list                  every HID interface that is a pad, XInput users, WinMM joysticks, and what the
//!                                   input layer makes of them (player, backend)
//!     nxinput read [seconds=20]     prints each change of each player's buttons and sticks (guest bits after
//!                                   button_layout), and each physical button as both layouts name it
//!     nxinput rumble                lights on, low motor 0.4 s, high motor 0.4 s, everything off (under a second)
//!
//! `--layout xbox` / `--map south:a` set button_layout / button_map for the run.

use nxos::input::{self, layout, PADS, PLAYERS};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    for w in args.windows(2) {
        if w[0] == "--layout" {
            nxconfig::apply_arg(&format!("button_layout={}", w[1]));
        }
        if w[0] == "--map" {
            nxconfig::apply_arg(&format!("button_map={}", w[1]));
        }
    }
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("list");
    match cmd {
        "list" => list(),
        "read" => read(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20.0)),
        "rumble" => rumble(),
        _ => {
            eprintln!("usage: nxinput list | read [seconds] | rumble [--layout switch|xbox] [--map ...]");
            std::process::exit(2);
        }
    }
}

fn list() {
    let hid = input::hidio::enumerate();
    println!("HID interfaces: {} ({} game controllers)", hid.len(), hid.iter().filter(|i| i.is_gamepad()).count());
    for i in hid.iter().filter(|i| i.is_gamepad() || input::sony::model(i.vid, i.pid) != 0 || input::nintendo::kind(i.vid, i.pid) != 0) {
        println!(
            "  {:04x}:{:04x} v{:04x} usage {:x}:{:x} in {} out {} feature {} {} \"{}\" sony={} nintendo={}",
            i.vid,
            i.pid,
            i.version,
            i.usage_page,
            i.usage,
            i.input_len,
            i.output_len,
            i.feature_len,
            if i.bluetooth { "Bluetooth" } else { "USB" },
            i.product,
            input::sony::model(i.vid, i.pid),
            input::nintendo::kind(i.vid, i.pid)
        );
    }
    println!("XInput ({}):", input::xinput::dll().unwrap_or("not loaded"));
    for u in 0..4 {
        match input::xinput::state(u) {
            Some(g) => println!("  user {u}: connected, buttons {:#06x} sticks {} {} {} {}", g.buttons, g.lx, g.ly, g.rx, g.ry),
            None => println!("  user {u}: none"),
        }
    }
    match input::winmm::open(0) {
        Some(j) => println!("WinMM: joystick {} {:04x}:{:04x} ({} axes, {} buttons)", j.id, j.vid, j.pid, j.axes, j.buttons),
        None => println!("WinMM: no joystick answers"),
    }
    // Switch 2 controllers over Bluetooth LE: the watcher, a few seconds of advertisements
    input::switch2_ble::start();
    for _ in 0..150 {
        input::probe_tick();
        std::thread::sleep(Duration::from_millis(20));
    }
    let d = input::devices();
    println!("input layer: {} device(s)", d.len());
    for (n, p, b, a) in d {
        println!("  {n}: player {} buttons {b:#06x} sticks {a:?}", p.map(|p| (p + 1).to_string()).unwrap_or("-".into()));
    }
}

fn read(secs: f64) {
    input::switch2_ble::start();
    let t0 = Instant::now();
    let mut last = [(u64::MAX, [0i32; 4]); PLAYERS];
    let mut ndev = usize::MAX;
    let (sw, _, _) = layout::build("switch", "");
    let (xb, _, _) = layout::build("xbox", "");
    let phys = ["right", "bottom", "top", "left", "lstick", "rstick", "l", "r", "zl", "zr", "plus", "minus", "dleft", "dup", "dright", "ddown"];
    let mut last_raw = vec![];
    println!("reading for {secs} s: press buttons, move the sticks");
    while t0.elapsed().as_secs_f64() < secs {
        input::probe_tick();
        let d = input::devices();
        if d.len() != ndev {
            ndev = d.len();
            println!("[{:6.2}] {} device(s): {:?}", t0.elapsed().as_secs_f64(), ndev, d.iter().map(|x| format!("{} (player {:?})", x.0, x.1.map(|p| p + 1))).collect::<Vec<_>>());
        }
        let raw: Vec<u64> = d.iter().map(|x| x.2).collect();
        if raw != last_raw {
            for (i, (n, _, b, _)) in d.iter().enumerate() {
                let old = last_raw.get(i).copied().unwrap_or(0);
                for bit in 0..16 {
                    if b >> bit & 1 != 0 && old >> bit & 1 == 0 {
                        let g = |m: &layout::PadMap| layout::GUEST[(layout::apply(m, 1 << bit).trailing_zeros() as usize).min(15)];
                        println!("[{:6.2}] {n}: {:<7} -> switch: {:<6} xbox: {}", t0.elapsed().as_secs_f64(), phys[bit], g(&sw), g(&xb));
                    }
                }
            }
            last_raw = raw;
        }
        for p in 0..PLAYERS {
            let s = PADS[p].peek();
            if s != last[p] && (last[p].0 != u64::MAX || s != (0, [0; 4])) {
                println!("[{:6.2}] player {} guest buttons {:#06x} sticks {:?}", t0.elapsed().as_secs_f64(), p + 1, s.0, s.1);
            }
            last[p] = s;
        }
        let m = input::motion();
        if t0.elapsed().as_millis() % 1000 < 5 {
            for p in 0..PLAYERS {
                let s = m.pad[p].lock().unwrap();
                if s.fresh(input::motion::now_s()) {
                    let x = s.at(input::motion::now_s());
                    println!("[{:6.2}] player {} motion acc {:+.2} {:+.2} {:+.2} G gyro {:+.3} {:+.3} {:+.3} rev/s", t0.elapsed().as_secs_f64(), p + 1, x.acc[0], x.acc[1], x.acc[2], x.gyro[0], x.gyro[1], x.gyro[2]);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
}

fn rumble() {
    for _ in 0..30 {
        input::probe_tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    let d = input::devices();
    if d.is_empty() {
        println!("no controllers");
        return;
    }
    println!("{} device(s); lights on, low 0.4 s, high 0.4 s, off", d.len());
    let run = |v: input::Vib, ms: u64| {
        for p in 0..PLAYERS {
            input::vibrate(p, 0, v);
            input::vibrate(p, 1, v);
        }
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            input::probe_tick();
            std::thread::sleep(Duration::from_millis(4));
        }
    };
    run(input::Vib { amp_low: 0.5, ..Default::default() }, 400);
    run(input::Vib { amp_high: 0.5, ..Default::default() }, 400);
    run(input::Vib::default(), 50);
    input::shutdown();
    println!("done");
}
