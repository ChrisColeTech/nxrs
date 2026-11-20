//! docs/game-patches.md, the index of game-specific code (nxnative `dm/tools/docs_patches.py` for this engine's
//! profiles): per game, its patch tables (nxnative's `patches.toml`, the owner's `turbo.toml`, a `recomp.txt` table)
//! with each patch's build, module, place and reason, whether nxrs builds apply it and the switch that turns it off;
//! the binding notes (why a patch is bound the way it is); and the profile keys that move run-time behaviour away
//! from its default. Then the patch sets shared by many games. Written from the profiles themselves, so the page is
//! the data; `game_patches_page_is_current` checks it.

use crate::patches::PatchFile;

/// The profile keys nxrs reads at run time (the rest: identity, packaging, and nxnative's Apple-only keys such as
/// `ipa`, `zip_store`, `shader_versions`, stay out of the index).
const RUNTIME: &[&str] = &[
    "fps_target", "fps_max", "fps_hold", "present_interval", "op_mode", "perf_mode", "core_preempt", "alloc_big_align", "shared_font", "device_api", "pad_layout", "game_args",
    "game_env", "launch_param", "render_base", "ue_texture_pool", "ue_fps_cvar", "draw_barriers", "tex_dyn_slots", "shader_key_skip", "stdalloc_cap", "tex_flush_upload",
    "tex_dirty_upload", "vconv_range", "screen_aspect",
];

fn table(out: &mut String, title: &str, f: &PatchFile) {
    let n = f.cut.len() + f.obs.len() + f.stub.len() + f.matches.len();
    if n == 0 && f.binding.is_empty() {
        return;
    }
    out.push_str(&format!("\n### {title} ({})\n\n", f.source));
    if !f.cut.is_empty() {
        out.push_str("| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |\n|---|---|---|---|---|---|---|\n");
        for (i, c) in f.cut.iter().enumerate() {
            out.push_str(&format!(
                "| {} | `{}` | {} | {:#x} | {} | {} | {} |\n",
                i + 1,
                short(&c.build),
                c.module.as_deref().unwrap_or("main"),
                c.offset,
                if c.apply.as_deref() == Some("build") { "yes" } else { "no (data)" },
                c.env.as_deref().map(|e| format!("`{e}=0`")).unwrap_or_default(),
                cell(&c.reason)
            ));
        }
    }
    if !f.obs.is_empty() {
        out.push_str("\n| Observation | Build | Offset | Callback | Reason |\n|---|---|---|---|---|\n");
        for (i, o) in f.obs.iter().enumerate() {
            out.push_str(&format!("| {} | `{}` | {:#x} | {} | {} |\n", i + 1, short(&o.build), o.offset, o.number, cell(&o.reason)));
        }
    }
    if !f.stub.is_empty() {
        out.push_str("\n| Stub | Kind | Build | Sites | Puts |\n|---|---|---|---|---|\n");
        for (i, s) in f.stub.iter().enumerate() {
            let puts: Vec<String> = s.body.puts.iter().map(|p| cell(&p.reason)).filter(|r| !r.is_empty()).collect();
            out.push_str(&format!("| {} | {} | `{}` | {} | {} |\n", i + 1, s.kind, short(&s.build), s.body.sites.len(), puts.join("; ")));
        }
    }
    if !f.matches.is_empty() {
        out.push_str("\n| Matched rule | Kind | Module | Function size |\n|---|---|---|---|\n");
        for m in &f.matches {
            out.push_str(&format!("| {} | {} | {} | {} |\n", cell(&m.name), m.kind, m.module, m.size.map(|s| format!("{s:#x}")).unwrap_or_default()));
        }
    }
    if !f.binding.is_empty() {
        out.push_str("\nBindings:\n\n");
        for b in &f.binding {
            out.push_str(&format!("- **{}**: bound {} (goal: {}); {}{}\n", cell(&b.what), cell(&b.now), cell(&b.goal), cell(&b.why), b.blocked.as_deref().map(|x| format!(" Blocked: {}.", cell(x))).unwrap_or_default()));
        }
    }
}

fn short(build: &str) -> String {
    build.chars().take(16).collect()
}

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ").trim().to_string()
}

/// The page.
pub fn game_patches_markdown() -> String {
    let mut out = String::from(
        "# Game patches\n\nThe game-specific code and settings of every profile (`src/profiles/games/<id>/`), written from the profiles by\n`profiles::docs::game_patches_markdown` (`cargo test -p profiles game_patches_page_is_current` checks it): the patch tables with\neach patch's place and reason, whether nxrs builds apply it and the switch that turns it off, the binding notes, and the\nprofile keys that change run-time behaviour. Edit the profiles, not this page.\n",
    );
    let mut games: Vec<&crate::Profile> = crate::all().iter().collect();
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    let mut listed = 0;
    for p in games {
        let keys: Vec<String> = p
            .rows
            .iter()
            .filter(|(k, _)| RUNTIME.contains(&k.as_str()))
            .map(|(k, v)| format!("`{k}` {}", v.join(" ")))
            .collect();
        let has = p.patches.is_some() || p.turbo.is_some() || p.recomp.is_some() || !keys.is_empty();
        if !has {
            continue;
        }
        listed += 1;
        out.push_str(&format!("\n## {} (`{}`)\n", p.name, p.id));
        if !keys.is_empty() {
            out.push_str(&format!("\nProfile keys: {}\n", keys.join(", ")));
        }
        if let Some(f) = &p.patches {
            table(&mut out, "nxnative's patches", f);
        }
        if let Some(f) = &p.turbo {
            table(&mut out, "the owner's patches (Turbo)", f);
        }
        if p.recomp.is_some() {
            out.push_str("\n### A code patch table (`recomp.txt`)\n\nApplied by nxrs builds of the module build it names.\n");
        }
    }
    out.push_str("\n## Shared patch sets\n");
    for f in crate::shared_patches() {
        table(&mut out, "shared", f);
    }
    out.push_str(&format!("\n({listed} games listed.)\n"));
    out
}

#[cfg(test)]
mod tests {
    /// docs/game-patches.md is what `game_patches_markdown` writes (a stale page is written again, then the test fails
    /// once so the change is seen).
    #[test]
    fn game_patches_page_is_current() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/game-patches.md");
        let want = super::game_patches_markdown();
        assert!(want.contains("## Shared patch sets"));
        let have = std::fs::read_to_string(&p).unwrap_or_default().replace("\r\n", "\n");
        if have != want {
            std::fs::write(&p, &want).unwrap();
            panic!("docs/game-patches.md was out of date and has been written again: check it in");
        }
    }
}
