//! egui's font families from the system's fonts, a CJK face in every family, egui's bundled fonts last.

use crate::services::font_files::FontFiles;
use egui::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

pub fn definitions(files: FontFiles) -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    let mut add = |name: &str, file: Option<(Vec<u8>, u32)>| -> bool {
        let Some((bytes, index)) = file else {
            return false;
        };
        let mut data = FontData::from_owned(bytes);
        data.index = index;
        defs.font_data.insert(name.into(), Arc::new(data));
        true
    };
    let (ui, semi, mono, cjk) = (
        add("ui", files.ui),
        add("ui-semibold", files.semibold),
        add("ui-mono", files.mono),
        add("cjk", files.cjk),
    );
    let symbols = add("symbols", files.symbols);
    let mono_bold = add("ui-mono-bold", files.mono_bold);
    let builtin = |f: FontFamily| defs.families.get(&f).cloned().unwrap_or_default();
    let (prop, mono_builtin) = (
        builtin(FontFamily::Proportional),
        builtin(FontFamily::Monospace),
    );
    let family = |first: Option<&str>, rest: &[String]| {
        let mut list: Vec<String> = first.map(String::from).into_iter().collect();
        if cjk {
            list.push("cjk".into());
        }
        if symbols {
            list.push("symbols".into());
        }
        list.extend(rest.iter().cloned());
        list
    };
    let semi_first = if semi {
        Some("ui-semibold")
    } else if ui {
        Some("ui")
    } else {
        None
    };
    let (p, s, m) = (
        family(ui.then_some("ui"), &prop),
        family(semi_first, &prop),
        family(mono.then_some("ui-mono"), &mono_builtin),
    );
    // the wordmark: the bold mono face, else the mono family
    let bold_first = if mono_bold {
        Some("ui-mono-bold")
    } else {
        mono.then_some("ui-mono")
    };
    let mb = family(bold_first, &mono_builtin);
    defs.families
        .insert(FontFamily::Name(super::theme::MONO_BOLD.into()), mb);
    defs.families.insert(FontFamily::Proportional, p);
    defs.families
        .insert(FontFamily::Name(super::theme::SEMIBOLD.into()), s);
    defs.families
        .insert(FontFamily::Name(super::theme::MONO.into()), m.clone());
    defs.families.insert(FontFamily::Monospace, m);
    defs
}
