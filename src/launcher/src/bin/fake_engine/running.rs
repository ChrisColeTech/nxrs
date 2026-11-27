//! The fake engine's answers that run until they are ended: `sleep` (a minute, for the cancel
//! tests) and `pad-bridge` (WEB-PADBRIDGE: each stdin line echoed into the bridge's log, then
//! `fake pad bridge: end` when stdin closes).

pub fn sleep() -> i32 {
    std::thread::sleep(std::time::Duration::from_secs(60));
    0
}

pub fn pad_bridge() -> i32 {
    let lines = std::io::stdin().lines().map_while(Result::ok);
    lines.for_each(|l| println!("fake pad bridge: {l}"));
    println!("fake pad bridge: end");
    0
}
