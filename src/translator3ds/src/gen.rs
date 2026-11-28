//! the nxrs-side generate: a title's programs as C under a build dir, overrides and the host-run natives
//! included, hints seeded in. The W0 form: no compile, no install, no data staging — those are the
//! pipeline's (3DS-W1). The donor's build.rs::generate is the shape this follows; its lines are kept where
//! they name what happened.

use std::path::{Path, PathBuf};

use crate::codegen;
use crate::discover::{self, Analysis, Mode, Source};
use crate::hints;
use crate::natives;
use crate::overrides::{self, Override};
use crate::rom::Title;
use crate::Mods;

/// what happens along the way; the pipeline's progress lines build on these (3DS-W1)
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// the C got written
    Generated {
        files: usize,
        bytes: usize,
        overrides: usize,
        /// functions discovery found across the title's programs
        functions: usize,
    },
    /// smth worth knowing that doesn't stop it
    Note(String),
}

/// what a generate should do
pub struct GenOptions3ds<'a> {
    /// where the C goes
    pub dir: &'a Path,
    /// C files of handwritten funcs, see docs/overrides.md
    pub overrides: Option<&'a Path>,
    /// addrs the last run had to interpret, odd for Thumb; where they're read from is the pipeline's (3DS-W1)
    pub hints: Vec<u32>,
}

/// the title's code as C
pub struct Generated {
    pub dir: PathBuf,
    /// files to compile, names inside dir
    pub sources: Vec<String>,
    pub files: usize,
    pub bytes: usize,
}

/// writes the title's programs as C in dir, handwritten overrides and the C library's divisions included,
/// plus what mods swap in for the title's own code and files
pub fn generate(
    title: &Title,
    mods: Mods,
    options: GenOptions3ds,
    events: &(dyn Fn(Event) + Sync),
) -> Result<Generated, String> {
    let mut programs = crate::programs(title, mods)
        .map_err(|error| format!("could not read the code, {error}"))?;
    let dir = options.dir;
    let files = match options.overrides {
        Some(path) => overrides::load(path)?,
        None => Vec::new(),
    };
    let mut replaced: Vec<Override> = files
        .iter()
        .flat_map(|file| file.overrides.iter().cloned())
        .collect();
    let by_hand = replaced.len();
    // the C lib's divisions run on the host as their own overrides, unless a handwritten one takes the addr
    // modules call the exe's
    let natives: Vec<natives::Native> = natives::find(&programs[0].1.text)
        .into_iter()
        .filter(|native| {
            !replaced
                .iter()
                .any(|item| item.module.is_none() && item.address == native.address)
        })
        .collect();
    let native_source = (!natives.is_empty()).then(|| natives::source(&natives));
    if let Some(source) = &native_source {
        replaced.extend(overrides::parse(source)?);
        events(Event::Note(format!(
            "{} of the C library's divisions run on the host",
            natives.len()
        )));
    }
    // whatever an override replaces still gets generated, so it can call it
    for item in &replaced {
        let name = item.module.as_deref().unwrap_or("executable");
        let Some((_, program)) = programs.iter_mut().find(|(program, _)| program == name) else {
            return Err(format!("the title has no module called {name}"));
        };
        program.seeds.insert(0, (item.address, Source::Override));
    }

    let mut analyses: Vec<Analysis> = programs
        .iter()
        .map(|(_, program)| discover::analyze(program))
        .collect();
    let functions = hints::apply_hints(&options.hints, &mut programs, &mut analyses);
    if functions > 0 {
        events(Event::Note(format!(
            "{functions} of the places the last run interpreted start new functions"
        )));
    }
    for item in &replaced {
        let index = programs
            .iter()
            .position(|(name, _)| Some(name.as_str()) == item.module.as_deref())
            .unwrap_or(0);
        let thumb = item.address & 1 != 0;
        let found = analyses[index]
            .functions
            .values()
            .any(|f| (f.mode == Mode::Thumb) == thumb && f.labels.contains(&(item.address & !1)));
        if !found {
            events(Event::Note(format!(
                "no code at 0x{:08X} for {} to replace, so it cannot call the original",
                item.address, item.name
            )));
        }
    }
    let units: Vec<codegen::Unit> = programs
        .iter()
        .zip(&analyses)
        .enumerate()
        .map(|(i, ((name, program), analysis))| codegen::Unit {
            module: (i > 0).then_some(name.as_str()),
            program,
            analysis,
        })
        .collect();
    let generated = codegen::generate(&units, &replaced);

    let write = |path: &Path, contents: &[u8]| {
        std::fs::write(path, contents)
            .map_err(|error| format!("could not write {}, {error}", path.display()))
    };
    let folder = if files.is_empty() {
        dir.to_path_buf()
    } else {
        dir.join("hand")
    };
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("could not create {}, {error}", folder.display()))?;
    for (name, contents) in &generated {
        write(&dir.join(name), contents.as_bytes())?;
    }
    let mut sources: Vec<String> = generated
        .iter()
        .map(|(name, _)| name.clone())
        .filter(|name| name.ends_with(".c"))
        .collect();
    for file in &files {
        let name = format!(
            "hand/{}",
            file.path.file_name().unwrap_or_default().to_string_lossy()
        );
        write(&dir.join(&name), file.source.as_bytes())?;
        sources.push(name);
    }
    if let Some(source) = &native_source {
        write(&dir.join("natives.c"), source.as_bytes())?;
        sources.push("natives.c".to_owned());
    }
    let bytes = generated.iter().map(|(_, contents)| contents.len()).sum();
    let functions = analyses.iter().map(|analysis| analysis.functions.len()).sum();
    events(Event::Generated {
        files: generated.len(),
        bytes,
        overrides: by_hand,
        functions,
    });
    Ok(Generated {
        dir: dir.to_path_buf(),
        sources,
        files: generated.len(),
        bytes,
    })
}
