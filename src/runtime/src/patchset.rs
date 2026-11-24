//! The game's code patch sets at run time: the generated code's `rt_patch_on` table (one byte a set: its sites run
//! the patched instruction while it is non-zero) and each set's environment switch name, handed over by the game's
//! entry (`nxrt_patch_table`, before the guest starts) so the host can flip a set while the game runs (os `cheats.rs`:
//! a code cheat's toggle). A build from before the hand-over has no table: nothing can be flipped.

use std::sync::Mutex;

struct Table {
    on: *mut u8,
    envs: Vec<String>,
}
// SAFETY: the table is the game's static array; it lives as long as the process.
unsafe impl Send for Table {}

static TABLE: Mutex<Option<Table>> = Mutex::new(None);

/// `void nxrt_patch_table(uint8_t *on, const char *const *envs, int count)`: the generated code's switch table and
/// the sets' switch names (count entries; a null name is a set without one).
///
/// # Safety
/// `on` points to at least `count` bytes that live as long as the process; `envs` to `count` C strings or nulls.
#[no_mangle]
pub unsafe extern "C" fn nxrt_patch_table(
    on: *mut u8,
    envs: *const *const core::ffi::c_char,
    count: i32,
) {
    if on.is_null() || count <= 0 {
        return;
    }
    let names = (0..count as usize)
        .map(|i| {
            let p = if envs.is_null() {
                core::ptr::null()
            } else {
                *envs.add(i)
            };
            if p.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        })
        .collect();
    *TABLE.lock().unwrap() = Some(Table { on, envs: names });
}

/// Whether a set of that switch name exists in the game's code.
pub fn has(env: &str) -> bool {
    TABLE
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|t| t.envs.iter().any(|e| e == env))
}

/// Turns the set of that switch name on or off; false when there is none.
pub fn set(env: &str, on: bool) -> bool {
    let g = TABLE.lock().unwrap();
    let Some(t) = g.as_ref() else { return false };
    let Some(i) = t.envs.iter().position(|e| e == env) else {
        return false;
    };
    // SAFETY: i < count (the table's length); a byte store the generated code reads at each site
    unsafe { core::ptr::write_volatile(t.on.add(i), on as u8) };
    true
}

/// The set's state, None when there is none.
pub fn get(env: &str) -> Option<bool> {
    let g = TABLE.lock().unwrap();
    let t = g.as_ref()?;
    let i = t.envs.iter().position(|e| e == env)?;
    // SAFETY: as in `set`
    Some(unsafe { core::ptr::read_volatile(t.on.add(i)) } != 0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn handed_over_table_flips() {
        static mut ON: [u8; 3] = [0, 1, 0];
        let names = [
            c"NXRS_MOD_A".as_ptr(),
            core::ptr::null(),
            c"NXRS_CHEAT_0000ABCD".as_ptr(),
        ];
        unsafe {
            super::nxrt_patch_table(core::ptr::addr_of_mut!(ON) as *mut u8, names.as_ptr(), 3)
        };
        assert!(super::has("NXRS_CHEAT_0000ABCD") && !super::has("NXRS_CHEAT_FFFFFFFF"));
        assert!(super::set("NXRS_CHEAT_0000ABCD", true));
        assert_eq!(unsafe { ON[2] }, 1);
        assert_eq!(super::get("NXRS_MOD_A"), Some(false));
        assert!(!super::set("nope", true));
    }
}
