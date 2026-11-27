# WEB-W11 - Module size

Wave of `docs/35-WEB-TARGET-PLAN.md` W11 ("Module size"). Base `f37a15a`, branch `web-w11`, worktree
`D:\Projects\RecompLauncherEngine-web-w11`. Title: Beach Bounce Remastered (01004060145C0000), library entry output
name `Beach Bounce Remastered nsw2u`. Chrome on GPU 1 only (`--use-adapter-luid`), one game Chrome on the machine.
Protocol: `docs/31-DELEGATION-PROTOCOL.md`. Evidence: `build/WEB/W11/`.

## 1. Plan

1. **The launcher did not see Beach Bounce's web build.** The launcher's web folder is
   `<game folder>\nxrs-web\<outputName>` (`model::web_build::web_dir`, the same `-o` it passes to the build); the
   build in the library was made by hand with `-o "Beach Bounce Remastered"` (WEB-W7) and sat in
   `nxrs-web\Beach Bounce Remastered`. A CLI build without `-o` names its folder after the package's NPDM name
   (`pipeline::default_web_outdir`), which the launcher's output name (from the file name) need not equal. Fix:
   the folder renamed and rebuilt into the right place; Play and the "web build" fact look for the game's own folder
   first and otherwise for a web build beside it whose `build.json` names the same title id
   (`services::web_serve::played_web_dir`), so a hand build under another name is still found.
2. **Measure** where the bytes go (`build/WEB/W11/tools/wasmsize.py`: sections, functions by size, duplicate bodies;
   data segments; the web engine library's data per crate).
3. **Try each technique on its own**, keep what pays: unreachable guest functions at translation, identical-function
   folding (wasm-opt), wasm-opt levels against time, compile flags, names, data/table compaction.
4. **Verify**: cli/translator/profiles tests, the size table before/after, one Chrome run of the final module to the
   story scenes (as WEB-W7 run7), compile+instantiate time from the page log, FPS.

## 2. Expected files

| File | Change |
|---|---|
| `src/launcher/src/services/web_serve.rs`, `build_facts.rs`, `flows/web.rs`, `tests/web_found.rs` | item 1 |
| `src/profiles/build.rs` | data the web engine does not need left out (found in item 2) |
| `src/cli/src/cc.rs`, `pipeline.rs`, `websplit.rs` | link / wasm-opt choices that pay |
| `src/translator/*` | only if unreachable-function elimination pays |
| `docs/35-WEB-TARGET-PLAN.md` | the W11 row |
