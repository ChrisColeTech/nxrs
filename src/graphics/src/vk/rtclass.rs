//! The real-time thread class of the threads that carry a frame (nxnative `nvn_rt.h` and hle_nvn.c rt_tick).
//!
//! nxnative declares a time-constraint policy on each such thread (macOS / iOS): the period of a frame and the CPU time
//! the thread needs in it, measured over a window of `WIN` presented frames (the larger of 1.25 x the p90 and 1.1 x the
//! peak, between 1 ms and 0.9 x the period; declared again only when it moves by more than 15%). Windows has no
//! time-constraint policy: its counterpart is the Multimedia Class Scheduler's "Games" task, which raises the thread
//! into the scheduler's multimedia class (as games' render threads do). Each frame thread joins it (`join`), and the
//! computation it needs is measured with the same window and logged when it moves (`tick`, once per piece of work):
//! the thread policy line says what each thread needs of its period.
//!
//! The frame threads: the replay thread (`vk replay`) and the game's thread that acquires window textures.
//! `NXGFX_RT_THREAD=0` leaves every thread in its normal class.

use std::sync::atomic::{AtomicU64, Ordering};

pub const WIN: usize = 120;

/// One thread's window (nxnative `nvn_rt`).
#[derive(Clone, Debug)]
pub struct Rt {
    /// the frame count and the thread's CPU ns at its last sample (frame 0: none yet)
    pub frame: u64,
    pub cpu: u64,
    pub win: [f64; WIN],
    pub n: usize,
    /// the computation declared (ms; 0: none yet)
    pub comp_ms: f64,
}

impl Default for Rt {
    fn default() -> Rt {
        Rt {
            frame: 0,
            cpu: 0,
            win: [0.0; WIN],
            n: 0,
            comp_ms: 0.0,
        }
    }
}

/// The computation a window asks for, in ms.
pub fn need(win: &[f64], period_ms: f64) -> f64 {
    let mut v = win.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    let p90 = v[((n * 9) / 10).min(n - 1)];
    let c = (p90 * 1.25).max(v[n - 1] * 1.1).min(period_ms * 0.9);
    c.max(1.0)
}

impl Rt {
    /// One sample on the thread itself: `frame` the count of presented frames, `cpu_ns` the thread's CPU time. The
    /// CPU since the last sample counts for the frames presented since, as one value (their mean). Returns the
    /// computation to declare now (ms), 0 for no change.
    pub fn sample(&mut self, frame: u64, cpu_ns: u64, period_ms: f64) -> f64 {
        if self.frame == 0 || frame < self.frame {
            self.frame = frame;
            self.cpu = cpu_ns;
            return 0.0;
        }
        if frame == self.frame {
            return 0.0;
        }
        self.win[self.n] =
            cpu_ns.saturating_sub(self.cpu) as f64 / 1e6 / (frame - self.frame) as f64;
        self.n += 1;
        self.frame = frame;
        self.cpu = cpu_ns;
        if self.n < WIN {
            return 0.0;
        }
        self.n = 0;
        let c = need(&self.win, period_ms);
        if self.comp_ms == 0.0 || c > self.comp_ms * 1.15 || c < self.comp_ms * 0.85 {
            c
        } else {
            0.0
        }
    }
}

fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let v = std::env::var("NXGFX_RT_THREAD").as_deref() != Ok("0");
        if !v {
            crate::vklog!("NXGFX_RT_THREAD=0: frame threads stay in their normal scheduling class");
        }
        v
    })
}

/// The thread's CPU time (ns).
fn thread_cpu_ns() -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
        let z = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        if unsafe { GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u) } == 0 {
            return 0;
        }
        let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) * 100;
        ft(k) + ft(u)
    }
    #[cfg(target_os = "linux")]
    {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }
}

/// The period of a frame the threads are measured against (ns; the present interval's).
pub static PERIOD_NS: AtomicU64 = AtomicU64::new(16_666_667);

struct Thr {
    name: &'static str,
    r: Rt,
    joined: bool,
}

thread_local! {
    static THR: std::cell::RefCell<Option<Thr>> = const { std::cell::RefCell::new(None) };
}

/// This thread carries a frame's CPU work: it joins the multimedia "Games" class (once). On the thread itself.
pub fn join(name: &'static str) {
    THR.with(|t| {
        let mut t = t.borrow_mut();
        if t.is_some() {
            return;
        }
        let joined = on() && enter_class();
        crate::vklog!(
            "{name}: {}",
            if joined {
                "joined the multimedia class scheduler's Games task"
            } else {
                "stays in its normal class"
            }
        );
        *t = Some(Thr {
            name,
            r: Rt::default(),
            joined,
        });
    });
}

#[cfg(windows)]
fn enter_class() -> bool {
    use windows_sys::Win32::System::Threading::AvSetMmThreadCharacteristicsW;
    let task: Vec<u16> = "Games\0".encode_utf16().collect();
    let mut idx = 0u32;
    let h = unsafe { AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut idx) };
    !h.is_null()
}
#[cfg(not(windows))]
fn enter_class() -> bool {
    false
}

/// Before a piece of the thread's work: one sample a presented frame; the computation it needs, logged when it moves.
pub fn tick() {
    THR.with(|t| {
        let mut t = t.borrow_mut();
        let Some(t) = t.as_mut() else { return };
        let f = super::present::frames();
        if f == t.r.frame {
            return;
        }
        let period = PERIOD_NS.load(Ordering::Relaxed) as f64 / 1e6;
        let c = t.r.sample(f, thread_cpu_ns(), period);
        if c > 0.0 {
            t.r.comp_ms = c;
            static N: AtomicU64 = AtomicU64::new(0);
            if N.fetch_add(1, Ordering::Relaxed) < 40 {
                crate::vklog!(
                    "{} needs {c:.2} ms of CPU a frame of {period:.2} ms{}",
                    t.name,
                    if t.joined { " (Games class)" } else { "" }
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// n frames of `ms` each, one sample a frame: the last non-zero declaration (0: none). (nxnative t_thread_rt.c)
    fn feed(r: &mut Rt, f: &mut u64, cpu: &mut u64, n: usize, ms: f64, period: f64) -> f64 {
        let mut got = 0.0;
        for _ in 0..n {
            *f += 1;
            *cpu += (ms * 1e6) as u64;
            let c = r.sample(*f, *cpu, period);
            if c > 0.0 {
                got = c;
                r.comp_ms = c;
            }
        }
        got
    }

    #[test]
    fn frame_thread_computation_follows_the_window() {
        let (mut r, mut f, mut cpu) = (Rt::default(), 10u64, 0u64);
        r.sample(f, cpu, 33.3);
        assert_eq!(
            feed(&mut r, &mut f, &mut cpu, WIN - 1, 10.0, 33.3),
            0.0,
            "nothing before a whole window"
        );
        let c = feed(&mut r, &mut f, &mut cpu, 1, 14.0, 33.3);
        assert!(
            c > 15.3 && c < 15.5,
            "10 ms frames with one of 14 declare 1.1 x the peak: {c}"
        );
        assert_eq!(
            feed(&mut r, &mut f, &mut cpu, WIN, 11.0, 33.3),
            0.0,
            "13.75 ms is within 15%: no new declaration"
        );
        let c = feed(&mut r, &mut f, &mut cpu, WIN, 6.0, 33.3);
        assert!(
            c > 7.4 && c < 7.6,
            "6 ms frames declare 1.25 x the p90: {c}"
        );
        let (mut r, mut f, mut cpu) = (Rt::default(), 1u64, 0u64);
        r.sample(f, cpu, 33.3);
        let c = feed(&mut r, &mut f, &mut cpu, WIN, 40.0, 33.3);
        assert!(c > 29.9 && c < 30.0, "at most 0.9 x the period: {c}");
        assert_eq!(
            feed(&mut r, &mut f, &mut cpu, WIN, 0.1, 33.3),
            1.0,
            "at least 1 ms"
        );
        let (mut r, mut cpu) = (Rt::default(), 0u64);
        r.sample(100, cpu, 33.3);
        cpu += 50_000_000;
        r.sample(200, cpu, 33.3);
        assert!(
            r.n == 1 && r.win[0] > 0.49 && r.win[0] < 0.51,
            "100 frames over 50 ms: one sample of 0.5 ms"
        );
        r.sample(200, cpu + 1_000_000, 33.3);
        assert_eq!(r.n, 1, "a second sample in the same frame adds nothing");
    }

    /// The class on this machine: a thread joins MMCSS's Games task, and its CPU time is read.
    #[test]
    fn a_frame_thread_joins_the_games_class() {
        std::thread::spawn(|| {
            join("test frame thread");
            let joined = THR.with(|t| t.borrow().as_ref().map(|t| t.joined));
            #[cfg(windows)]
            assert_eq!(joined, Some(true), "AvSetMmThreadCharacteristicsW(Games)");
            #[cfg(not(windows))]
            let _ = joined;
            // the thread's CPU time moves (Windows counts it in scheduler quanta: spin until it does)
            let (a, t0) = (thread_cpu_ns(), std::time::Instant::now());
            let mut x = 0u64;
            while thread_cpu_ns() == a && t0.elapsed().as_secs() < 5 {
                x = std::hint::black_box(x.wrapping_add(1));
            }
            assert!(thread_cpu_ns() > a, "the thread's CPU time moves");
        })
        .join()
        .unwrap();
    }
}
