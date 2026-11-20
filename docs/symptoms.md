# Symptoms

What a player or developer sees, and the reference entry that explains it. Written by `scripts/docs_symptoms.py` from the
`- **Symptom:**` bullets of [the reference docs](reference/README.md); edit those, not this page.

## [packing](reference/packing.md)

- Mario Kart 8 Deluxe hangs when an installed DLC title's data is missing. ([DLC with missing data offered to the game](reference/packing.md#dlc-with-missing-data-offered-to-the-game))
- the build ends with `<name>_code.dll is in use. Close the game and try again.` ([Link fails with "is in use"](reference/packing.md#link-fails-with-is-in-use))

## [rewriter](reference/rewriter.md)

- Pokémon Scarlet / Violet or Legends: Z-A stops for good while its main thread waits. ([Trinity rendezvous hang](reference/rewriter.md#trinity-rendezvous-hang))

## [memory](reference/memory.md)

- Pokémon Legends: Z-A aborts in the SDK (nn::diag) during `nninitStartup`. ([Alias region too small](reference/memory.md#alias-region-too-small))
- Mario Kart World aborts at main's DT_INIT with 0xE801; Donkey Kong Bananza's SDK aborts at 0.9 s. ([Memory attributes refused](reference/memory.md#memory-attributes-refused))
- Windows runs out of memory while a game compiles pipelines (Ocarina of Time, 1 GB a second). ([Driver allocations take the machine down](reference/memory.md#driver-allocations-take-the-machine-down))

## [threads](reference/threads.md)

- Mario Kart World crashed 15.5 s into a run. ([Core count from the NPDM](reference/threads.md#core-count-from-the-npdm))

## [services](reference/services.md)

- Mario Kart 8 Deluxe crashes in `nn::mii::CreateId::IsValid` after the Mii editor applet. ([Mii editor result read as a new Mii](reference/services.md#mii-editor-result-read-as-a-new-mii))
- Pokémon Violet 4.0.0 aborts at start. ([Bluetooth interfaces missing at start](reference/services.md#bluetooth-interfaces-missing-at-start))
- Ocarina of Time's network library (NPLN) aborts at start. ([Event descriptor sockets](reference/services.md#event-descriptor-sockets))
- Mario Kart World calls through a null object after `nn::chat::Initialize`. ([Unknown Switch 2 services](reference/services.md#unknown-switch-2-services))
- a scripted run of a new game stops at name entry (PORT_PLAN M6: Pokémon Violet). ([Name entry in scripted runs](reference/services.md#name-entry-in-scripted-runs))

## [movies](reference/movies.md)

- Mario Kart 8 Deluxe's mode-select preview panel draws black although the H.264 pictures decode fine. ([MK8D's menu preview panel](reference/movies.md#mk8ds-menu-preview-panel))

## [audio](reference/audio.md)

- Ocarina of Time's opening is silent. ([Voices that reach no mix](reference/audio.md#voices-that-reach-no-mix))
- silence where a game plays Opus audio through the hardware decoder. ([Opus decoded to silence](reference/audio.md#opus-decoded-to-silence))

## [frame-rate](reference/frame-rate.md)

- frames held for two vsyncs after a single long frame (nxnative issue #1823, Mario Kart World's ([One stall raised the hold](reference/frame-rate.md#one-stall-raised-the-hold))
- Donkey Kong Bananza stays at its smallest render size. ([Idle gaps read as GPU time](reference/frame-rate.md#idle-gaps-read-as-gpu-time))
- Super Mario Bros. Wonder stutters (60-2,000 ms frames) entering new places. ([Slow frames at every new place](reference/frame-rate.md#slow-frames-at-every-new-place))

## [nvn-objects](reference/nvn-objects.md)

- Mario Kart 8 Deluxe shows black stretches with the asynchronous replay. ([A texture released while queued work reads it](reference/nvn-objects.md#a-texture-released-while-queued-work-reads-it))
- Mario Kart World stalls 31-40 s into a run. ([Window acquire waits](reference/nvn-objects.md#window-acquire-waits))

## [rendering](reference/rendering.md)

- Mario Kart 8 Deluxe shows black stretches with the asynchronous replay. ([Asynchronous replay black stretches](reference/rendering.md#asynchronous-replay-black-stretches))
- Mario Kart 8 Deluxe's world draws black. ([Black world from ignored render conditions](reference/rendering.md#black-world-from-ignored-render-conditions))
- Mario Kart World's menu text is missing. ([Glyph uploads lost](reference/rendering.md#glyph-uploads-lost))
- the karts' shadow blob in Mario Kart 8 Deluxe is cyan instead of dark. ([Cyan kart shadow (open)](reference/rendering.md#cyan-kart-shadow-open))
- Super Mario Party Jamboree shows a white layer over the picture. ([White layer (open)](reference/rendering.md#white-layer-open))
- the device is lost in Mario Kart World. ([Mesh-draw faults (open)](reference/rendering.md#mesh-draw-faults-open))
- colour banding in Anime Girls: Sun of a Beach. ([Banding (open)](reference/rendering.md#banding-open))

## [shaders](reference/shaders.md)

- Super Mario Bros. Wonder's frames take 60-2,000 ms at every new place. ([Pipelines made at every new place](reference/shaders.md#pipelines-made-at-every-new-place))
- Mario Kart 8 Deluxe's race stays black after the course loads. ([Draws skipped while their shader is translated](reference/shaders.md#draws-skipped-while-their-shader-is-translated))
- ASTC's error colour on streamed sparse textures (nxnative's Densha de GO!! case). ([LOD clamp dropped](reference/shaders.md#lod-clamp-dropped))

## [cheats](reference/cheats.md)

- a cheat that patches code has no effect, and the log says in red that it was not compiled in. ([Code cheat not compiled in](reference/cheats.md#code-cheat-not-compiled-in))

## [game-state](reference/game-state.md)

- a scripted run stops at a name entry screen. ([Name entry in a scripted run](reference/game-state.md#name-entry-in-a-scripted-run))
- Super Mario Party Jamboree shows a white layer over the picture. ([Jamboree's white layer](reference/game-state.md#jamborees-white-layer))
- Donkey Kong Bananza runs at about 20 FPS on a fast PC. ([Donkey Kong Bananza's 20 FPS](reference/game-state.md#donkey-kong-bananzas-20-fps))
- a long scripted run of Pokémon Legends: Arceus stops progressing at a story point. ([Story points that need a hand play](reference/game-state.md#story-points-that-need-a-hand-play))
- a game waits at a "connect controllers" screen. ([Two players asked for, one connected](reference/game-state.md#two-players-asked-for-one-connected))
- A and B seem swapped on an Xbox or PlayStation pad. ([Buttons by position](reference/game-state.md#buttons-by-position))
