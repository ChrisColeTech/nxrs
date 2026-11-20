# Cheats and mods

Atmosphère-format cheat codes and the player's mods, and where nxrs has to differ from a Switch because the game's
code is translated ahead of time. The run-time cheat engine is `src/os/src/cheats.rs`; build-time code cheats are
`src/cli/src/cheats.rs` with the shared file parser `src/profiles/src/cheatfile.rs`; mods are
`src/profiles/src/mods.rs` (parsing), `src/cli/src/mods.rs` (build) and `src/os/src/mods.rs` (run).

## How cheats work on a Switch

On a console running Atmosphère, the `dmnt` sysmodule reads `atmosphere/contents/<title id>/cheats/<build id>.txt`,
where the build id is the first 16 hex digits of the main module's build id. A file holds entries: `[Name]` a cheat,
`{Name}` the master code (always on), each with 32-bit code words. dmnt runs every enabled cheat in a small VM, repeatedly, against
the game's memory: main, heap, alias and ASLR regions, with 16 registers, conditionals,
loops, key tests and stores. A failed access does not crash the game. The reference is Atmosphère's
`dmnt_cheat_vm.cpp` and its own `docs/features/cheats.md` (in Atmosphère's repository), which nxrs follows.

## The VM as nxrs runs it

`src/os/src/cheats.rs`:

- **Files.** `<data>\cheats\<build>.txt`, or the Atmosphère tree
  `<data>\cheats\atmosphere\contents\<title>\cheats\<build>.txt`; `NXOS_CHEATS=<folder>` names another folder. An
  entry with no code words is a label. A header glued to the previous line's last word still starts a new entry.
- **When.** One pass at every present (the OS layer's present hook), not on a fixed timer: registers reset, the master
  codes run, then the enabled cheats in file order, sharing registers. Each cheat runs as its own program, so an
  unclosed conditional cannot swallow the next cheat.
- **Opcodes.** 0-A and C0-C4 with the reference's word order, conditions, loops and register semantics. Any other
  opcode turns that cheat off with the reason ("unsupported opcode").
- **Memory types.** The memory-type nybble: 0 main, 1 heap, 2 alias, 3 ASLR, 4 absolute, 5-F main.
- **Failures.** An unmapped page or a pointer not set yet is ignored as on a Switch: a load leaves the register, a
  condition compares 0, a store writes nothing; the first failure of a cheat logs one line.

## Toggles, values and the panel

- `<build>.enabled` beside the file: one enabled cheat name per line, rewritten when toggles change.
- `NXOS_CHEATS_ON=<name>,...` enables cheats at launch.
- Control commands: `cheats` (list with state), `cheats reload` (read the file again), `cheat on|off <name>`,
  `cheat set <name> <value>`.
- `<build>.meta.tsv` (nxnative's format, written beside a compiled file by its `cheat_anchor.py`): per entry its
  section, note, type, on / off pairs, an "ask" value the player may set (kept in `<build>.values`), "set" cheats that
  run one pass and turn themselves off, and never-decrease (a host action keeping a typed value at its highest).
  Columns are read by header name; "unresolved" rows are listed.
- The panel: F6, or Home + Minus on a controller, opens a small window over the game listing the cheats with their
  state and values (up / down move, A or Return toggles, left / right step a value, B or Escape closes). While it is
  open the game reads no pads and is out of focus, as under the console's menus.

## What this host does differently: code cheats

A cheat that stores into main's instructions changes bytes the translated game never executes. So the build compiles
such cheats in:

- `nxrs-build` reads the same cheat file for main's build (`src/cli/src/cheats.rs`). A cheat qualifies when every
  instruction is opcode 0 (store a static value) into main (memory type 0) with the offset register taken as 0
  (`profiles::cheatfile`).
- Each qualifying cheat becomes a patch set of its own, `cheat_<hash>`, switch `NXRS_CHEAT_<hash>`, off by default (a
  master code: on). Both the original and the patched instruction are lifted ([rewriter.md](rewriter.md#patch-table)).
- At run time the game hands its switch table to the runtime (`nxrt_patch_table`, `src/runtime/src/patchset.rs`);
  toggling the cheat flips its set as well as running its stores.
- A cheat with any logic stays the VM's alone; its writes reach the bytes only.

### Code cheat not compiled in

- **Symptom:** a cheat that patches code has no effect, and the log says in red that it was not compiled in.
- **Cause:** the cheat file was added (or changed) after the build.
- **Fix:** rebuild with the file in place; the build adds the cheat's set.

## Mods

A mod folder takes the layouts Atmosphère, Ryujinx and yuzu use, so a downloaded mod drops in unchanged
(`src/profiles/src/mods.rs`). Mods live in `<build folder>\mods\<mod>\` (`NXRS_MODS=<folder>` names another); nxrs
never ships or downloads one.

```text
mods/<mod>/mod.json                   optional {"name", "enabled", "description"}
mods/<mod>/exefs/<build id>.ips       IPS / IPS32 by module build id
mods/<mod>/exefs/*.pchtxt             IPSwitch text patches (@nsobid-<build id>)
mods/<mod>/exefs_patches/<any>/...    Atmosphère's layout
mods/<mod>/*.ips | *.pchtxt           loose patch files
mods/<mod>/romfs/...                  files that replace or add to the RomFS
```

IPS offsets count from the start of the NSO file (0x100-byte header), so image offset = offset - 0x100; `.pchtxt`
offsets are as written plus `@flag offset_shift`, then the same 0x100 comes off.

- **Build** (`src/cli/src/mods.rs`): code patches become switchable sites, one set per mod, on by default. Data bytes
  and the RomFS layer go to `<data>\mods.tsv` (`mod <switch> <default> <folder> <romfs or ->` and
  `bytes <switch> <module> <offset> <hex>` lines).
- **Run** (`src/os/src/mods.rs`): a mod is on unless `NXRS_MOD_<NAME>=0` or a `disabled` file is in its folder
  (`NXRS_MOD_<NAME>=1` wins over the file), else as its `mod.json` says. Enabled data bytes are written into the
  loaded modules before any guest code runs; its `romfs\` files are layered over the game's RomFS.
- A mod added after the build needs a rebuild: `mods.tsv` and the switchable sites are made at build time. Turning
  a built mod on or off needs no rebuild.

## Sources

Atmosphère 1.x `stratosphere/dmnt/source/cheat/impl/dmnt_cheat_vm.cpp` and `docs/features/cheats.md` (paths in Atmosphère's repository); nxnative's
`hle_cheats.c` for the host design (reimplemented, not copied).
