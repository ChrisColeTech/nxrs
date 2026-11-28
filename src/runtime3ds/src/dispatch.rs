//! the title's generated code as this host sees it: the DLL loaded (LoadLibraryW), its `recomp_*`
//! tables read (the donor `abi/src/lib.rs` Library's shape), the entry binary search with stale
//! filtering, CRO module bases written where the code reads them, the origins checked against
//! memory, and the natives switch.

use crate::context::{self, Code, Entry, Module, NO_START, Origins};
use std::ffi::c_void;

#[cfg(windows)]
struct Dll(*mut c_void);

#[cfg(windows)]
impl Dll {
    fn load(path: &std::path::Path) -> Result<Dll, String> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::LoadLibraryW;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // safe: the module handle stays in the Dll; generated code is never unloaded, it lives
        // for the process like the tables and fn pointers handed out of it
        let module = unsafe { LoadLibraryW(wide.as_ptr()) };
        if module.is_null() {
            return Err(format!("could not load {}", path.display()));
        }
        Ok(Dll(module))
    }

    fn symbol(&self, name: &str) -> Result<*const c_void, String> {
        use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
        let cname = std::ffi::CString::new(name).map_err(|e| e.to_string())?;
        // safe: GetProcAddress on a live module
        let address = unsafe { GetProcAddress(self.0, cname.as_ptr() as *const u8) }
            .ok_or_else(|| format!("the code has no {name}"))?;
        Ok(address as *const c_void)
    }
}

#[cfg(not(windows))]
struct Dll;

#[cfg(not(windows))]
impl Dll {
    fn load(_path: &std::path::Path) -> Result<Dll, String> {
        Err("the code library loads on Windows hosts".into())
    }
    fn symbol(&self, _name: &str) -> Result<*const c_void, String> {
        Err("no symbols without a loaded library".into())
    }
}

/// one unit's origins with its tables copied out, so check works without holding raw pointers
struct Unit {
    /// 0 is the exe, i + 1 is module i
    index: usize,
    pieces: Vec<context::Piece>,
    functions: Vec<context::Origin>,
    runs: Vec<context::Run>,
}

pub struct Library {
    dll: Option<Dll>,
    entries: *const Entry,
    count: usize,
    modules: *const Module,
    module_count: usize,
    /// modules placed somewhere, as base, size and index
    placed: Vec<(u32, u32, usize)>,
    /// interface version the code was made for
    pub abi: u32,
    /// codegen version that made the code, 0 for libs older than that
    pub generation: u32,
    origins: *const Origins,
    /// recomp_natives, the C lib funcs the code runs on the host; null when there are none
    natives: *mut u8,
    units: Vec<Unit>,
}

// safe for the same reasons the donor's mirror names: the tables live as long as the library,
// only the running thread touches the bases and the flags
unsafe impl Send for Library {}
unsafe impl Sync for Library {}

impl Library {
    /// loads a DLL a build compiled from the generated C
    pub fn open(path: &std::path::Path) -> Result<Library, String> {
        let dll = Dll::load(path)?;
        let word = |name: &str| -> Result<u32, String> {
            let address = dll.symbol(name)?;
            // safe: recomp_abi and friends are const u32s
            Ok(unsafe { *(address as *const u32) })
        };
        let table = |name: &str| -> Result<*const c_void, String> { dll.symbol(name) };
        let abi = word("recomp_abi")?;
        if !(context::OLDEST_ABI..=context::ABI).contains(&abi) {
            return Err(format!(
                "it was built for version {abi} of the interface, this is {}",
                context::ABI
            ));
        }
        let entries = table("recomp_entries")? as *const Entry;
        let count = word("recomp_entry_count")? as usize;
        let modules = table("recomp_modules")? as *const Module;
        let module_count = word("recomp_module_count")? as usize;
        let generation = table("recomp_generation")
            .ok()
            .map(|generation| unsafe { *(generation as *const u32) })
            .unwrap_or(0);
        let origins = table("recomp_origins")
            .ok()
            .filter(|_| generation >= 3)
            .map(|origins| origins as *const Origins)
            .unwrap_or(std::ptr::null());
        let natives = table("recomp_natives")
            .ok()
            .map(|natives| natives as *mut u8)
            .unwrap_or(std::ptr::null_mut());
        let units: Vec<Unit> = (0..=module_count)
            .map(|index| unsafe { read_unit(origins, index) })
            .collect();
        Ok(Library {
            dll: Some(dll),
            entries,
            count,
            modules,
            module_count,
            placed: Vec::new(),
            abi,
            generation,
            origins,
            natives,
            units,
        })
    }

    /// # Safety
    ///
    /// the pointers must be the recomp_ tables of code outliving the Library (the linked-in form
    /// a 3DS build ships, W3's pipeline); tests use it for hand-made tables
    pub unsafe fn linked(
        abi: u32,
        entries: *const Entry,
        count: usize,
        modules: *const Module,
        module_count: usize,
        origins: *const Origins,
        generation: u32,
    ) -> Library {
        let units: Vec<Unit> = (0..=module_count)
            .map(|index| unsafe { read_unit(origins, index) })
            .collect();
        Library {
            dll: None,
            entries,
            count,
            modules,
            module_count,
            placed: Vec::new(),
            abi,
            generation,
            origins,
            natives: std::ptr::null_mut(),
            units,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        // safe bc the table lives as long as the code and count says how long it is
        unsafe { std::slice::from_raw_parts(self.entries, self.count) }
    }

    /// # Safety
    ///
    /// the module table lives as long as the code
    pub unsafe fn modules(&self) -> &[Module] {
        unsafe { std::slice::from_raw_parts(self.modules, self.module_count) }
    }

    pub fn module_index(&self, name: &str) -> Option<usize> {
        // safe bc the table lives as long as the code
        unsafe { self.modules().iter().position(|module| module.name() == name) }
    }

    /// how many funcs and modules there's code for
    pub fn describe(&self) -> String {
        format!("{} entry points, {} modules", self.count, self.module_count)
    }

    /// tells module index's code where it got loaded, 0 when it goes away
    pub fn place(&mut self, index: usize, base: u32) {
        // safe bc the module's var is the code's own, read once per entry
        let size = unsafe { (*self.modules.add(index)).size };
        unsafe { (*self.modules.add(index)).place(base) };
        self.placed.retain(|&(_, _, i)| i != index);
        if base != 0 {
            self.placed.push((base, size, index));
        }
    }

    /// where module index sits (0 when it does not)
    pub fn base_of(&self, index: usize) -> u32 {
        self.placed.iter().find(|&&(_, _, i)| i == index).map(|&(base, _, _)| base).unwrap_or(0)
    }

    /// switches the C lib funcs the code runs on the host (natives) back to the title's own code
    /// when off; says whether the code has any, they start on
    pub fn set_natives(&mut self, on: bool) -> bool {
        if self.natives.is_null() {
            return false;
        }
        // safe bc recomp_natives is the code's own byte, only the thread running it reads it
        unsafe { *self.natives = on as u8 };
        true
    }

    fn find(&self, unit: usize, entries: &[Entry], address: u32) -> Option<Code> {
        let index = entries.binary_search_by_key(&address, |entry| entry.address).ok()?;
        let stale = self.origins(unit).is_some_and(|origins| origins.owner_is_stale(index));
        (!stale).then_some(entries[index].code)
    }

    /// code that can run from address (bit 0 for Thumb) in the exe or a placed module,
    /// none for a func whose code changed since it was made
    pub fn lookup(&self, address: u32) -> Option<Code> {
        self.find(0, self.entries(), address).or_else(|| {
            let &(base, _, index) =
                self.placed.iter().find(|&&(base, size, _)| address.wrapping_sub(base) < size)?;
            // safe bc the module table lives as long as the code
            let entries = unsafe { (*self.modules.add(index)).entries() };
            self.find(index + 1, entries, address.wrapping_sub(base))
        })
    }

    /// what unit's funcs came from, None for code that doesn't say
    pub fn origins(&self, unit: usize) -> Option<&Origins> {
        if self.origins.is_null() || unit > self.module_count {
            return None;
        }
        // safe bc the code has a table for the exe and each module
        Some(unsafe { &*self.origins.add(unit) })
    }

    /// checks unit's funcs against memory (read fills it in from an addr, offsets plus base);
    /// funcs w/ changed bytes go stale and stop running, returns how many
    pub fn check(&mut self, unit: usize, base: u32, mut read: impl FnMut(u32, &mut [u8])) -> usize {
        let Some(unit) = self.units.get(unit) else { return 0 };
        let Some(origins) = self.origins(unit.index) else { return 0 };
        let (Some(first), Some(last)) = (unit.pieces.first(), unit.pieces.last()) else { return 0 };
        let mut code = vec![0u8; (last.end - first.start) as usize];
        read(base.wrapping_add(first.start), &mut code);
        // how many pieces before each one changed
        let mut changed_before = Vec::with_capacity(unit.pieces.len() + 1);
        changed_before.push(0u32);
        for piece in &unit.pieces {
            let mut hash = context::CodeHash::default();
            hash.add(&code[(piece.start - first.start) as usize..(piece.end - first.start) as usize]);
            let changed = (hash.value() != piece.hash) as u32;
            changed_before.push(changed_before.last().unwrap() + changed);
        }
        let mut stale = 0;
        for (index, function) in unit.functions.iter().enumerate() {
            let runs = &unit.runs[function.first as usize..(function.first + function.count) as usize];
            let changed = runs
                .iter()
                .any(|run| changed_before[run.end as usize] > changed_before[run.first as usize]);
            // safe bc the flags and starts are the code's own, one each per func
            unsafe {
                *origins.stale.add(index) = changed as u8;
                *origins.starts.add(index) = if changed { NO_START } else { function.start };
            }
            stale += changed as usize;
        }
        stale
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // the module is never unloaded: generated code is process-lifetime, the fn pointers in
        // the Entry tables live with it
        let _ = self.dll.take();
    }
}

/// reads a unit's tables out of the code (copied, so check works without holding raw pointers)
///
/// # Safety
///
/// origins must be a valid recomp_origins table, index within it
unsafe fn read_unit(origins: *const Origins, index: usize) -> Unit {
    if origins.is_null() {
        return Unit { index, pieces: Vec::new(), functions: Vec::new(), runs: Vec::new() };
    }
    // safe bc the caller's pointer is a live table
    let table = unsafe { &*origins.add(index) };
    let pieces = unsafe { table.pieces() }.to_vec();
    let functions = unsafe { table.functions() }.to_vec();
    let runs = unsafe { table.runs() }.to_vec();
    Unit { index, pieces, functions, runs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Entry, Module, Origin, Piece, Run};

    unsafe extern "C" fn the_code(_ctx: *mut crate::context::Context) -> u32 {
        0
    }
    unsafe extern "C" fn other_code(_ctx: *mut crate::context::Context) -> u32 {
        0
    }

    fn entry(address: u32, code: Code) -> Entry {
        Entry { address, code }
    }

    /// hand-made tables behave like generated ones: search, stale, no origins at all
    #[test]
    fn a_linked_table_searches_and_stales() {
        let entries = [entry(0x0010_0000, the_code), entry(0x0010_0010, other_code)];
        let owners = [0u32, 1];
        let mut stale = [0u8; 2];
        let mut starts = [0x0010_0000u32, 0x0010_0010];
        let origins = Origins {
            count: 0,
            piece_count: 0,
            pieces: std::ptr::null(),
            functions: std::ptr::null(),
            runs: std::ptr::null(),
            owners: owners.as_ptr(),
            stale: stale.as_mut_ptr(),
            starts: starts.as_mut_ptr(),
        };
        let library = unsafe {
            Library::linked(context::ABI, entries.as_ptr(), 2, std::ptr::null(), 0, &origins, context::GENERATION)
        };
        assert!(library.lookup(0x0010_0000).is_some());
        assert!(library.lookup(0x0010_0008).is_none());
        assert!(library.lookup(0x0010_0010).is_some());
        // a stale func's entries stop answering
        stale[0] = 1;
        assert!(library.lookup(0x0010_0000).is_none());
        assert!(library.lookup(0x0010_0010).is_some(), "the other func's entry is its own");
        assert_eq!(stale, [1, 0]);
        // a table without origins searches the same way
        let plain = unsafe {
            Library::linked(context::ABI, entries.as_ptr(), 2, std::ptr::null(), 0, std::ptr::null(), 0)
        };
        assert!(plain.lookup(0x0010_0000).is_some());
    }

    /// a module placed at a base answers lookups at base + offset, and nowhere else
    #[test]
    fn a_placed_module_answers_at_its_base() {
        let module_entries = [entry(0x200, other_code)];
        let base = Box::leak(Box::new(0u32)) as *mut u32;
        let module = Module {
            name: b"Battle\0".as_ptr() as *const core::ffi::c_char,
            base,
            size: 0x400,
            count: 1,
            entries: module_entries.as_ptr(),
        };
        let modules = [module];
        let entries = [entry(0x0010_0000, the_code)];
        let mut library =
            unsafe { Library::linked(context::ABI, entries.as_ptr(), 1, modules.as_ptr(), 1, std::ptr::null(), 0) };
        assert!(library.lookup(0x0040_0200).is_none());
        library.place(0, 0x0040_0000);
        assert_eq!(library.base_of(0), 0x0040_0000);
        assert_eq!(unsafe { *base }, 0x0040_0000, "the code's own var now says where it is");
        assert!(library.lookup(0x0040_0200).is_some());
        assert!(library.lookup(0x0040_0600).is_none(), "past its size");
        library.place(0, 0);
        assert_eq!(unsafe { *base }, 0);
        assert!(library.lookup(0x0040_0200).is_none());
    }

    /// check() marks exactly the funcs whose pieces changed, a piece hashed once how many funcs share it
    #[test]
    fn check_marks_the_functions_whose_code_changed() {
        let memory: Vec<u8> = (0..0x40u8).collect();
        let hash = |start: u32, end: u32| {
            let mut hash = context::CodeHash::default();
            hash.add(&memory[(start - 0x10) as usize..(end - 0x10) as usize]);
            hash.value()
        };
        let pieces = [
            Piece { start: 0x10, end: 0x20, hash: hash(0x10, 0x20) },
            Piece { start: 0x20, end: 0x28, hash: hash(0x20, 0x28) },
            Piece { start: 0x28, end: 0x30, hash: hash(0x28, 0x30) },
        ];
        let runs = [Run { first: 0, end: 3 }, Run { first: 1, end: 2 }, Run { first: 2, end: 3 }];
        let functions = [
            Origin { first: 0, count: 1, start: 0x10 },
            Origin { first: 1, count: 1, start: 0x20 },
            Origin { first: 2, count: 1, start: 0x28 },
        ];
        let owners = [0u32, 1, 2];
        let mut stale = [0u8; 3];
        let mut starts = [0x10u32, 0x20, 0x28];
        let origins = Origins {
            count: 3,
            piece_count: 3,
            pieces: pieces.as_ptr(),
            functions: functions.as_ptr(),
            runs: runs.as_ptr(),
            owners: owners.as_ptr(),
            stale: stale.as_mut_ptr(),
            starts: starts.as_mut_ptr(),
        };
        let entries: [Entry; 0] = [];
        let mut library = unsafe {
            Library::linked(context::ABI, entries.as_ptr(), 0, std::ptr::null(), 0, &origins, context::GENERATION)
        };
        let read = |memory: &[u8]| {
            let memory = memory.to_vec();
            move |address: u32, out: &mut [u8]| {
                out.copy_from_slice(&memory[(address - 0x10) as usize..][..out.len()])
            }
        };
        assert_eq!(library.check(0, 0, read(&memory)), 0);
        let mut changed = memory.clone();
        changed[0x2A - 0x10] ^= 1;
        assert_eq!(library.check(0, 0, read(&changed)), 2, "the first func holds the last piece too");
        assert_eq!(stale, [1, 0, 1]);
        assert_eq!(starts, [NO_START, 0x20, NO_START]);
    }
}
