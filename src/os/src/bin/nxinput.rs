//! nxinput: the controllers the engine sees, without a game.
//!
//!     nxinput list                  every HID interface that is a pad, XInput users, WinMM joysticks, and what the
//!                                   input layer makes of them (player, backend)
//!     nxinput read [seconds=20]     prints each change of each player's buttons and sticks (guest bits after
//!                                   button_layout), and each physical button as both layouts name it
//!     nxinput rumble [--short]      lights, low motor 1 s, high motor 1 s, both 1 s, off (--short: 0.4 s each, under a second)
//!     nxinput rumble --hd           HD rumble on a Pro Controller / Joy-Con (`rumble.rs`, the game's path): a note ladder in
//!                                   5 ms steps, a click train, then silence (about 3 s)
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
            eprintln!(
                "usage: nxinput list | read [seconds] | rumble [--layout switch|xbox] [--map ...]"
            );
            std::process::exit(2);
        }
    }
}

fn list() {
    let hid = input::hidio::enumerate();
    println!(
        "HID interfaces: {} ({} game controllers)",
        hid.len(),
        hid.iter().filter(|i| i.is_gamepad()).count()
    );
    for i in hid.iter().filter(|i| {
        i.is_gamepad()
            || input::sony::model(i.vid, i.pid) != 0
            || input::nintendo::kind(i.vid, i.pid) != 0
    }) {
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
            Some(g) => println!(
                "  user {u}: connected, buttons {:#06x} sticks {} {} {} {}",
                g.buttons, g.lx, g.ly, g.rx, g.ry
            ),
            None => println!("  user {u}: none"),
        }
    }
    match input::winmm::open(0) {
        Some(j) => println!(
            "WinMM: joystick {} {:04x}:{:04x} ({} axes, {} buttons)",
            j.id, j.vid, j.pid, j.axes, j.buttons
        ),
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
        println!(
            "  {n}: player {} buttons {b:#06x} sticks {a:?}",
            p.map(|p| (p + 1).to_string()).unwrap_or("-".into())
        );
    }
}

fn read(secs: f64) {
    input::switch2_ble::start();
    let t0 = Instant::now();
    let mut last = [(u64::MAX, [0i32; 4]); PLAYERS];
    let mut ndev = usize::MAX;
    let (sw, _, _) = layout::build("switch", "");
    let (xb, _, _) = layout::build("xbox", "");
    let phys = [
        "right", "bottom", "top", "left", "lstick", "rstick", "l", "r", "zl", "zr", "plus",
        "minus", "dleft", "dup", "dright", "ddown",
    ];
    let mut last_raw = vec![];
    println!("reading for {secs} s: press buttons, move the sticks");
    while t0.elapsed().as_secs_f64() < secs {
        input::probe_tick();
        let d = input::devices();
        if d.len() != ndev {
            ndev = d.len();
            println!(
                "[{:6.2}] {} device(s): {:?}",
                t0.elapsed().as_secs_f64(),
                ndev,
                d.iter()
                    .map(|x| format!("{} (player {:?})", x.0, x.1.map(|p| p + 1)))
                    .collect::<Vec<_>>()
            );
        }
        let raw: Vec<u64> = d.iter().map(|x| x.2).collect();
        if raw != last_raw {
            for (i, (n, _, b, _)) in d.iter().enumerate() {
                let old = last_raw.get(i).copied().unwrap_or(0);
                for bit in 0..16 {
                    if b >> bit & 1 != 0 && old >> bit & 1 == 0 {
                        let g = |m: &layout::PadMap| {
                            layout::GUEST
                                [(layout::apply(m, 1 << bit).trailing_zeros() as usize).min(15)]
                        };
                        println!(
                            "[{:6.2}] {n}: {:<7} -> switch: {:<6} xbox: {}",
                            t0.elapsed().as_secs_f64(),
                            phys[bit],
                            g(&sw),
                            g(&xb)
                        );
                    }
                }
            }
            last_raw = raw;
        }
        for p in 0..PLAYERS {
            let s = PADS[p].peek();
            if s != last[p] && (last[p].0 != u64::MAX || s != (0, [0; 4])) {
                println!(
                    "[{:6.2}] player {} guest buttons {:#06x} sticks {:?}",
                    t0.elapsed().as_secs_f64(),
                    p + 1,
                    s.0,
                    s.1
                );
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

/// `rumble` (or `--owner`): the physical check, strong and long enough to feel: lights,
/// low motor 160/255 1 s, high motor 160/255 1 s, both 200/255 1 s, stop. `rumble --short`: 128/255 0.4 s each
/// (under a second: the agents' bounded check). `rumble <low 0..255> <high 0..255> <ms>`: one setting for that long.
fn rumble() {
    let a: Vec<String> = std::env::args()
        .skip(2)
        .filter(|x| !x.starts_with("--layout") && !x.starts_with("--map"))
        .collect();
    for _ in 0..30 {
        input::probe_tick();
        std::thread::sleep(Duration::from_millis(10));
    }
    let d = input::devices();
    if d.is_empty() {
        println!("no controllers");
        return;
    }
    if a.iter().any(|x| x == "--hd") {
        return rumble_hd();
    }
    // the value is sent on every tick, as a game does (HD rumble's watchdog silences a side left unfed for 250 ms)
    let run = |low: u32, high: u32, ms: u64, what: &str| {
        println!("{what}: low {low}/255 high {high}/255 for {ms} ms");
        let v = input::Vib {
            amp_low: low as f32 / 255.0,
            amp_high: high as f32 / 255.0,
            ..Default::default()
        };
        let t = Instant::now();
        while t.elapsed() < Duration::from_millis(ms) {
            for p in 0..PLAYERS {
                input::vibrate(p, 0, v);
                input::vibrate(p, 1, v);
            }
            input::probe_tick();
            std::thread::sleep(Duration::from_millis(4));
        }
    };
    println!(
        "{} device(s): {:?}",
        d.len(),
        d.iter().map(|x| x.0.clone()).collect::<Vec<_>>()
    );
    if a.first().map(|s| s.as_str()) == Some("--short") {
        run(128, 0, 400, "low motor");
        run(0, 128, 400, "high motor");
    } else if let (Some(l), Some(h), Some(ms)) = (
        a.first().and_then(|x| x.parse().ok()),
        a.get(1).and_then(|x| x.parse().ok()),
        a.get(2).and_then(|x| x.parse().ok()),
    ) {
        run(l, h, ms, "setting");
    } else {
        println!(
            "Hold the controller: LOW motor (left grip), HIGH motor (right grip), both, then off."
        );
        run(0, 0, 1000, "lights on");
        run(160, 0, 1000, "low-frequency motor");
        run(0, 160, 1000, "high-frequency motor");
        run(200, 200, 1000, "both motors");
    }
    run(0, 0, 50, "off");
    input::shutdown();
    println!("done");
}

/// The scripted HD rumble envelope at `ms` into the script (None: the end).
fn hd_script(ms: u64) -> Option<(input::Vib, &'static str)> {
    let silent = input::Vib::default();
    match ms {
        // eight notes, 120 ms on (a decaying 5 ms-step envelope), 80 ms apart, 160 Hz up to 1000 Hz
        0..1600 => {
            let (k, t) = (ms / 200, ms % 200);
            if t >= 120 {
                return Some((silent, "note ladder"));
            }
            let f = 160.0 * (1000.0f32 / 160.0).powf(k as f32 / 7.0);
            let a = 0.7 * (1.0 - 0.6 * (t / 5 * 5) as f32 / 120.0);
            Some((
                input::Vib {
                    amp_low: 0.2 * a,
                    freq_low: (f / 2.0).max(41.0),
                    amp_high: a,
                    freq_high: f,
                },
                "note ladder",
            ))
        }
        // ten clicks: 10 ms full scale, 90 ms silent
        1600..2600 => Some((
            if (ms - 1600) % 100 < 10 {
                input::Vib {
                    amp_low: 0.5,
                    freq_low: 160.0,
                    amp_high: 1.0,
                    freq_high: 320.0,
                }
            } else {
                silent
            },
            "click train",
        )),
        2600..3000 => Some((silent, "silence")),
        _ => None,
    }
}

/// `rumble --hd`: the scripted envelope through the game's path (`input::vibrate` → `rumble.rs`), every 5 ms.
fn rumble_hd() {
    let t = Instant::now();
    loop {
        input::probe_tick();
        let (n, ready) = input::rumble::claimed();
        if (n > 0 && ready == n) || t.elapsed() > Duration::from_secs(3) {
            break;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    let (n, ready) = input::rumble::claimed();
    if n == 0 {
        println!("no Pro Controller / Joy-Con claimed for HD rumble (none connected, or hd_rumble=0): the script runs on the standard rumble");
    } else {
        println!("{n} pad(s) claimed for HD rumble, {ready} ready (NXRS_CONTROLLER_TRACE=1 logs each packet)");
    }
    let t0 = Instant::now();
    let mut phase = "";
    let mut step = 0u64;
    loop {
        let ms = t0.elapsed().as_millis() as u64;
        let Some((v, what)) = hd_script(step * 5) else {
            break;
        };
        if ms >= step * 5 {
            if what != phase {
                println!("[{:5} ms] {what}", step * 5);
                phase = what;
            }
            for p in 0..PLAYERS {
                input::vibrate(p, 0, v);
                input::vibrate(p, 1, v);
            }
            step += 1;
        }
        input::probe_tick();
        std::thread::sleep(Duration::from_millis(1));
    }
    // the watchdog and the stop packet, then the neutral packet at exit
    let t = Instant::now();
    while t.elapsed() < Duration::from_millis(400) {
        input::probe_tick();
        std::thread::sleep(Duration::from_millis(4));
    }
    input::shutdown();
    println!("done");
}
