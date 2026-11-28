//! the fast paths: GetSystemTick answered by the code itself (the tick_* fields) and the one
//! mutex the host lends the thread (wait_at/release_at/held/holding/held_floor), the way
//! docs/interface.md §Ticks and §A mutex lent design them. This host charges nothing for a
//! self-answered tick (tick_cost 0), counts the budget in ticks 1:1 and keeps no events mid
//! slice, so tick_floor 0 and held_floor 0 are safe; everything re-learns after each settle.

use crate::context::{self, Context};

/// claims the slice's tick window: tick_end - budget is the guest tick now
pub fn open_tick_window(ctx: &mut Context, slice_start_tick: u64, slice_budget: i32) {
    ctx.tick_end = slice_start_tick + slice_budget as u64;
    ctx.tick_cost = 0;
    ctx.tick_floor = 0;
    ctx.tick_at = context::NO_TICKS;
    ctx.tick_last = 0;
    ctx.ticks = 0;
}

/// the tick an svc answered now, as the code would compute it
pub fn tick_now(ctx: &Context) -> u64 {
    ctx.tick_end.wrapping_sub(ctx.budget as i32 as i64 as u64)
}

/// names the site the code just answered itself: the next tick there runs without the host
pub fn learn_tick_site(ctx: &mut Context, after: u32, thumb: bool) {
    ctx.tick_at = after | thumb as u32;
}

/// lends the mutex handle to this thread: the code answers its waits and releases itself at
/// the two sites until the next settle
pub fn lend_mutex(ctx: &mut Context, handle: u32, wait_after: u32, release_after: u32, thumb: bool) {
    ctx.held = handle;
    ctx.holding = ctx.holding.max(1);
    ctx.wait_at = wait_after | thumb as u32;
    ctx.release_at = release_after | thumb as u32;
    ctx.held_floor = 0;
}

/// the settle, at every boundary into the host loop: the mutex returns to the kernel exactly
/// as `holding` says, the ticks the code answered are counted, the loan and the counters reset
pub fn settle(ctx: &mut Context) -> Settled {
    let ticks = std::mem::take(&mut ctx.ticks);
    let trail_entries = std::mem::take(&mut ctx.done);
    let holding = std::mem::take(&mut ctx.holding);
    ctx.held = 0;
    ctx.wait_at = 0;
    ctx.release_at = 0;
    ctx.held_floor = context::NOTHING_HELD;
    ctx.tick_at = context::NO_TICKS;
    ctx.tick_last = 0;
    Settled { ticks_answered: ticks, trail_entries, releases_to_apply: holding }
}

/// what a settle hands the kernel
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settled {
    /// GetSystemTicks the code answered itself this run
    pub ticks_answered: u32,
    /// trail entries: the addrs-after of the svcs the code answered itself (ticks included)
    pub trail_entries: u32,
    /// how many times the code dropped the lent mutex; the kernel takes them back
    pub releases_to_apply: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thread::plain_context;

    /// the tick window reads as tick_end minus whatever budget is left
    #[test]
    fn the_tick_window_tracks_the_budget() {
        let mut ctx = plain_context(0);
        open_tick_window(&mut ctx, 1_000, 10_000);
        assert_eq!(ctx.tick_end, 11_000);
        ctx.budget = 7_777;
        assert_eq!(tick_now(&ctx), 11_000 - 7_777);
        assert_eq!(ctx.tick_cost, 0);
        assert_eq!(ctx.tick_floor, 0);
        assert_eq!(ctx.tick_at, context::NO_TICKS);
    }

    /// a learned tick site answers only there, and a settle clears it
    #[test]
    fn the_tick_site_relearns_after_a_settle() {
        let mut ctx = plain_context(0);
        learn_tick_site(&mut ctx, 0x0010_0004, true);
        assert_eq!(ctx.tick_at, 0x0010_0005, "bit 0 carries Thumb");
        let settled = settle(&mut ctx);
        assert_eq!(settled, Settled { ticks_answered: 0, trail_entries: 0, releases_to_apply: 0 });
        assert_eq!(ctx.tick_at, context::NO_TICKS);
    }

    /// a lend hands the handle and the two sites over, and a settle takes them back
    #[test]
    fn the_lent_mutex_settles_back() {
        let mut ctx = plain_context(0);
        ctx.holding = 0;
        lend_mutex(&mut ctx, 0x1234, 0x0010_0014, 0x0010_0020, false);
        assert_eq!(ctx.held, 0x1234);
        assert_eq!(ctx.holding, 1);
        assert_eq!(ctx.wait_at, 0x0010_0014);
        assert_eq!(ctx.release_at, 0x0010_0020);
        assert_eq!(ctx.held_floor, 0);
        ctx.holding = 3;
        ctx.done = 7;
        ctx.ticks = 11;
        let settled = settle(&mut ctx);
        assert_eq!(settled.releases_to_apply, 3);
        assert_eq!(settled.trail_entries, 7);
        assert_eq!(settled.ticks_answered, 11);
        assert_eq!(ctx.holding, 0);
        assert_eq!(ctx.held, 0);
        assert_eq!(ctx.held_floor, context::NOTHING_HELD);
    }
}
