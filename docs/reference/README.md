# Subsystem references

How nxrs-engine meets each part of a Switch or Switch 2 game on Windows x86-64: what each subsystem does, where it
lives, the switches and log lines used to debug it, and the problems found on games so far. Entries that describe a
visible problem start with a `- **Symptom:**` line. Settings are in [docs/settings.md](../settings.md), every
environment switch in [docs/env_switches.tsv](../env_switches.tsv), module APIs in
[docs/INTERFACES.md](../INTERFACES.md), open work in [docs/PORT_PLAN.md](../PORT_PLAN.md).

| File | Covers |
|---|---|
| [packing.md](packing.md) | Package input (folders, NSP, XCI, NCA, NUP), keys, updates, RomFS markers, DLC, run-time NROs, the `nxrs-build` pipeline and toolchain |
| [rewriter.md](rewriter.md) | The ARM64 to C translator: discovery, lowering, jump tables, exclusives, system registers, the patch table, `nso_match`, relocations, checking a change |
| [memory.md](memory.md) | Guest layout, heap and application pool, alias region, mapping SVCs, engine allocators, GPU-visible memory, the memory cap |
| [threads.md](threads.md) | Guest threads and virtual cores, locks and address arbitration, spin waits, events, fibers, stall dumps |
| [services.md](services.md) | Takeovers by name and IPC, gap lines, file system, input, keyboard and applets, display and operation mode, accounts, network, settings, time, NRO loading |
| [movies.md](movies.md) | The SDK's movie decoder, the Switch 2 video decoders, codecs, native images |
| [audio.md](audio.md) | The sound card, the SDK's renderer and the SDK 1.x host renderer, AudioOut / AudioIn, Opus |
| [frame-rate.md](frame-rate.md) | Present pacing, forced present intervals, the frame step, engine frame locks, frame generation, pacing metrics |
| [nvn-objects.md](nvn-objects.md) | NVN object layouts and sizes, device properties, memory pools, command memory, queues and syncs, entry points |
| [rendering.md](rendering.md) | The Vulkan replay: textures and formats, passes, state, draws, uniforms and ordering, queries, present, diagnostics |
| [shaders.md](shaders.md) | Shader containers, keys, build-time translation, bindings, load-time edits, the live translator, pipelines and caches |
| [cheats.md](cheats.md) | Atmosphère cheats, the run-time VM and panel, code cheats compiled in, mods |
| [game-state.md](game-state.md) | Saves, scripted state, game behaviour that is not an engine bug |

A new general finding goes into the file and section it belongs to, in the same form; a fact about one game goes
into that game's profile notes or the INTERFACES lane section that found it.
