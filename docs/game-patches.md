# Game patches

The game-specific code and settings of every profile (`src/profiles/games/<id>/`), written from the profiles by
`profiles::docs::game_patches_markdown` (`cargo test -p profiles game_patches_page_is_current` checks it): the patch tables with
each patch's place and reason, whether nxrs builds apply it and the switch that turns it off, the binding notes, and the
profile keys that change run-time behaviour. Edit the profiles, not this page.

## 13 Sentinels: Aegis Rim (`01003FC01670C000`)

Profile keys: `alloc_big_align` on

## 13 Sentinels: Aegis Rim (Asia) (`01008D7016438000`)

Profile keys: `alloc_big_align` on

## 13 Sentinels: Aegis Rim (Japan) (`010045C014650000`)

Profile keys: `alloc_big_align` on

## Absolum (`0100A83022A22000`)

Profile keys: `game_env` DOTNET_GCRegionRange=60000000, `stdalloc_cap` 0xa0000000

## Advance Wars 1+2: Re-Boot Camp (`0100300012F2A000`)

Profile keys: `pad_layout` full

## AI: THE SOMNIUM FILES (`010089B00D09C000`)

Profile keys: `pad_layout` full

## AI: THE SOMNIUM FILES - nirvanA Initiative (`0100713016CCC000`)

Profile keys: `pad_layout` full

## Animal Crossing: New Horizons (`01006F8002326000`)

Profile keys: `pad_layout` full, `screen_aspect` on

### nxnative's patches (nxnative dm/games/01006F8002326000/patches.py)


| Stub | Kind | Build | Sites | Puts |
|---|---|---|---|---|
| 1 | frame_step | `ff1d1c05670db602` | 14 | graphics create arg +0x60: the display size in the screen's shape (bit 0); graphics create arg +0x20: 1280 x the layout height (bit 1); platform config slot 7: the static display size in the screen's shape (bit 2); platform config slot 8: the static layout size 1280.0 x the layout height (bit 3); platform config slot 3: the arg's 1920x1080 in the screen's shape (bit 4); resolution table entry of the frame (crop, render scale): the height of the screen's shape (bit 7); resolution table entry of the frame (crop, render scale): the height of the screen's shape (bit 7); layouts' DrawInfo: the framebuffer's size in the screen's shape (bit 8); layouts' DrawInfo: the framebuffer's size in the screen's shape (bit 8); field camera: its vertical angle widened so the taller screen keeps 16:9's sides (bit 5); field camera: its vertical angle widened so the taller screen keeps 16:9's sides (bit 5); nn::ui2d::Pane::Calculate after the global matrix: duo_calc (the HUD at the taller screen's edges, full-screen backgrounds stretched); game camera: the vertical angle widened so the taller screen keeps 16:9's sides (bit 5); nn::ui2d::DrawInfo::SetProjectionMtx: the y row divided by the vertical stretch, layouts in their own shape (bit 6) |

Bindings:

- **screen shape: the sizes, the view, the layouts' projection and their edge widgets (DM_SCREEN_ASPECT)**: bound build (goal: game); the output size is the host's, but where the game takes its sizes, its cameras and its layouts are this build's code Blocked: the stubs have no function-matched form yet.

## ANIMAL WELL (`010020D01AD24000`)

Profile keys: `pad_layout` simple

## ASTRAL CHAIN (`01007300020FA000`)

Profile keys: `pad_layout` full, `fps_target` 60

### nxnative's patches (nxnative dm/games/01007300020FA000/patches.py)


| Stub | Kind | Build | Sites | Puts |
|---|---|---|---|---|
| 1 | frame_step | `4b159f0f7a360669` | 112 | frame step (0 = original): [hs] +0x2024 sibling path: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): S3 +0x2024 exact-zero (D1 handler 0xD3380) +0x2024: b 0xa0e8f0 (was subs w8,w8,#1); frame step (0 = original): 15-count (v1 site 3) +0x2020: b 0xa0e890 (was subs w8,w8,#1); frame step (0 = original): chain bind (v1 site 4) +0x2024: b 0xa0e8c0 (was subs w8,w8,#1); frame step (0 = original): S3 15-count in site-4 routine +0x2020: b 0xa0e860 (was subs w8,w8,#1); frame step (0 = original): +0xF64 short latch +0xf64: b 0xa0e920 (was subs w8,w8,#1); frame step (0 = original): bank +0x1364 buffer +0x1364: b 0xa0eaa0 (was sub w8,w8,#1); frame step (0 = original): bank +0x1368 buffer +0x1368: b 0xa0eac0 (was sub w8,w8,#1); frame step (0 = original): bank +0x136c buffer +0x136c: b 0xa0eae0 (was sub w8,w8,#1); frame step (0 = original): bank +0x138c buffer +0x138c: b 0xa0eb00 (was sub w8,w8,#1); frame step (0 = original): bank +0x1378 buffer +0x1378: b 0xa0eb20 (was sub w8,w8,#1); frame step (0 = original): bank +0x137c buffer +0x137c: b 0xa0eb40 (was sub w8,w8,#1); frame step (0 = original): bank +0x1390 buffer +0x1390: b 0xa0eb60 (was sub w8,w8,#1); frame step (0 = original): bank +0x1370 buffer +0x1370: b 0xa0eb80 (was sub w8,w8,#1); frame step (0 = original): bank +0x1374 buffer +0x1374: b 0xa0eba0 (was sub w8,w8,#1); frame step (0 = original): bank +0x13a4 buffer +0x13a4: b 0xa0ebc0 (was sub w8,w8,#1); frame step (0 = original): bank +0x13a8 buffer +0x13a8: b 0xa0ebe0 (was sub w8,w8,#1); frame step (0 = original): bank +0x13cc buffer +0x13cc: b 0xa0ec00 (was sub w8,w8,#1); frame step (0 = original): +0x14B8 buffer +0x14b8: b 0xa0e950 (was subs w8,w8,#1); frame step (0 = original): +0x46AEF0 tap buffer +0x46aef0: b 0xa0e980 (was subs w8,w8,#1); frame step (0 = original): bank +0x1470 buffer +0x1470: b 0xa0ec20 (was sub w8,w8,#1); frame step (0 = original): bank +0x1474 buffer +0x1474: b 0xa0ec40 (was sub w8,w8,#1); frame step (0 = original): bank +0x1478 buffer +0x1478: b 0xa0ec60 (was sub w8,w8,#1); frame step (0 = original): bank +0x147c buffer +0x147c: b 0xa0ec80 (was sub w8,w8,#1); frame step (0 = original): bank +0x1480 buffer +0x1480: b 0xa0eca0 (was sub w8,w8,#1); frame step (0 = original): bank +0x1484 buffer +0x1484: b 0xa0ecc0 (was sub w8,w8,#1); frame step (0 = original): bank +0x1488 buffer +0x1488: b 0xa0ece0 (was sub w8,w8,#1); frame step (0 = original): bank +0x148c buffer +0x148c: b 0xa0ed00 (was sub w8,w8,#1); frame step (0 = original): bank +0x1490 buffer +0x1490: b 0xa0ed20 (was sub w8,w8,#1); frame step (0 = original): bank +0x1450 buffer +0x1450: b 0xa0ed40 (was sub w8,w8,#1); frame step (0 = original): bank +0x1454 buffer +0x1454: b 0xa0ed60 (was sub w8,w8,#1); frame step (0 = original): [hs] +0x46AE28 window: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] +0x46ADAC exact-zero: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [isubs] +0x46B4C4 lockout: b cave 0xa0eea0 (was subs w10, w10, #1); frame step (0 = original): [up] +0x46B62C up-counter: b cave 0xa0eed0 (was add w10, w9, #1); frame step (0 = original): [hs] +0x14E0 window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x46AEF8 window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0xDA4 window (set 15 at 0x325300; next to r3 0x284AD4/0x284AF0): bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x46AF00 window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x46AF04 window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x46AE10 queue window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x46AE14 re-injected press window: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [upmin1] +0xC50 tap/hold count-up: b cave 0xa0eee4 (was add w9, w9, #1); frame step (0 = original): [hs] +0x14E0 window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x14E0 window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x14E0 window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): +0x14D4 buffer +0x14d4: b 0xa0e9b0 (was subs w8,w8,#1); frame step (0 = original): +0x14D8 buffer +0x14d8: b 0xa0e9e0 (was subs w8,w8,#1); frame step (0 = original): +0x1414 buffer +0x1414: b 0xa0ea10 (was subs w8,w8,#1); frame step (0 = original): +0x149C buffer +0x149c: b 0xa0ea40 (was subs w8,w8,#1); frame step (0 = original): +0x14DC buffer +0x14dc: b 0xa0ea70 (was subs w8,w8,#1); frame step (0 = original): [hs] +0x13A0 second-tap window, path A (b.ge -> 0x28772C mov w10,w9; str): bl stub_w10w9 0xa0efcc (was subs w9, w10, #1); frame step (0 = original): [hs] +0x14D4 r2-sibling path (shares store 0x2875A4 with r2 0x28759C): bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x13A0 second-tap window, path B (b.lt -> 0x287734): bl stub_w10w9 0xa0efcc (was subs w9, w10, #1); frame step (0 = original): [hs] +0x139C window (set 15 with bank +0x137C at 0x287764/0x287790): bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] +0x13B0 window (set 15 at 0x287830 next to bank +0x13A8 = 15): bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46a8b0: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46a8c4: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b3a8: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b3ac: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b460: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b464: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b490: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b49c: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b638: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b640: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b658: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b664: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b66c: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] lockout bank +0x46b39c: bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): combo cue (v1 site 1) +0x1290: b 0xa0e830 (was subs w8,w8,#1); frame step (0 = original): [hs] player deferred hitstop delay +0x7e88; exact-zero -> 0x91D6A0(w2=0): bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] player deferred hitstop delay +0x7e94; exact-zero -> 0x91D6A0(w2=0): bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): [hs] player deferred hitstop delay +0x7e80; exact-zero -> 0x91D6A0(w2=0): bl hs_w9 0xa0edbc (was subs w9, w9, #1); frame step (0 = original): was fmin s8,s0,s10 -> bl 0x5763E4; frame step (0 = original): was fmin s8,s1,s10 -> bl 0x5763EC; frame step (0 = original): was fcsel s8,s10,s9,gt -> bl 0x5763F4; frame step (0 = original): Replace FCVTZS: ldr w8,[x8,#0xD8]; frame step (0 = original): [hs] +0x38 delay set by 0x4F27A0(w3); exact-zero -> 0x4F27A0(+0x3C,+0x40) issues the scale: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x34 duration of scale +0x30; exact-zero -> sound 0x8DD890 + 0x923B90(+0xE8); expiry -> +0x30=1.0: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x48 duration of scale +0x44; exact-zero -> +0x14=0; expiry -> +0x44=1.0: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x74 duration of scale +0x70; exact-zero -> +0x10=0, 0x923B90(+0x100); expiry -> +0x70=1.0: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0xA8 request suppression window: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x48 sibling path after +0x34 expiry: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x74 sibling path after +0x48 expiry: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0xA8 request suppression window, b.lt path: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0xA4 duration of scale +0xA0: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x54 duration of scale +0x50: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x80 hold-request delay: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x54 sibling path after +0xA4 expiry: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [hs] +0x8C cue delay after request release; exact-zero -> sound 0x8DD890: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): [clamp1] +0x8C clamped decrement: b cave 0xa0ee70 (was sub w9, w9, #1); frame step (0 = original): [up] +0x84 freeze-phase count-up: b cave 0xa0ee8c (was add w8, w8, #1); frame step (0 = original): [ramp] manager +0x7C ramp step c*T: b cave 0xa0efac (was ldr s0, [x19, #0x7c]; 0x4F4070 now skipped); frame step (0 = original): [hs] +0xB4 window; expiry -> +0xAC=0: bl hs_w8 0xa0ed80 (was subs w8, w8, #1); frame step (0 = original): Load whole ticks: ldr w8,[x24,#0xD8]; frame step (0 = original): Zero ticks: cbz w8,0x57E810; frame step (0 = original): First sample: cbz x8,0xA0E828; frame step (0 = original): Measured sample: b 0xA0E7D0; frame step (0 = original): [isubs] clock +0x34 delay: b cave 0xa0ef04 (was subs w16, w16, #1); frame step (0 = original): [naive] clock +0x30 duration: b cave 0xa0ef34 (was subs w16, w16, #1); frame step (0 = original): [rumble] rumble channel 0 count +0x48: b stub 0xa0ef5c -> rh_w8 (was subs w9, w8, #1); frame step (0 = original): [rumble] rumble channel count +0x60: b stub 0xa0ef64 -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [rumble] rumble channel count +0x60: b stub 0xa0ef6c -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [rumble] rumble channel count +0x78: b stub 0xa0ef7c -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [rumble] rumble channel count +0x60: b stub 0xa0ef74 -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [rumble] rumble channel count +0x78: b stub 0xa0ef84 -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [rumble] rumble channel count +0x90: b stub 0xa0ef94 -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [rumble] rumble channel count +0x78: b stub 0xa0ef8c -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [rumble] rumble channel count +0x90: b stub 0xa0ef9c -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [rumble] rumble channel count +0x90: b stub 0xa0efa4 -> rh_w9 (was subs w10, w9, #1); frame step (0 = original): [up] pad auto-repeat count +0x30: b cave 0xa0ef48 (was add w9, w9, #1) |

Bindings:

- **60 fps frame step (112 sites)**: bound build (goal: game); the game's own clock and timers (a community 60 fps mod on the host's frame step). PlatinumGames' other games do not share them: none of its 15 functions has an exact match in Bayonetta, Bayonetta 2, Bayonetta 3 or NieR:Automata

## Atelier Ryza: Ever Darkness & the Secret Hideout DX (`0100199023B2E000`)

Profile keys: `pad_layout` full

## Balatro (`0100CD801CE5E000`)

Profile keys: `op_mode` handheld, `pad_layout` touch

## BALL x PIT (`010086A022444000`)

Profile keys: `pad_layout` full

## Batman: Arkham Knight (`0100ACD0163D0000`)

Profile keys: `shader_key_skip` 0x770+0x78

### nxnative's patches (nxnative dm/games/0100ACD0163D0000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `feae83dc9e9fb34c` | main | 0x192ec14 | no (data) |  | GMalloc setup: adrp x8 (first instruction of the inlined allocator constructor, 1 GB + 37 GB of address space) → b 0x192fea4 (__cxa_guard_release); the host allocator is in the object already (hle_uemalloc.c) |
| 2 | `feae83dc9e9fb34c` | main | 0xc5ae24 | no (data) |  | queue push: and x10, x10, #0xffff → and x11, x10, #0xffff (w10 keeps the tail) |
| 3 | `feae83dc9e9fb34c` | main | 0xc5ae28 | no (data) |  | queue push: str x8, [x9, x10, lsl #3] → str x8, [x9, x11, lsl #3] (the item) |
| 4 | `feae83dc9e9fb34c` | main | 0xc5ae2c | no (data) |  | queue push: ldr w8, [x19, #0x18] (the tail again) → dmb ishst |
| 5 | `feae83dc9e9fb34c` | main | 0xc5ae30 | no (data) |  | queue push: add w8, w8, #1 → add w8, w10, #1 (then str w8, [x19, #0x18]: the new tail) |
| 6 | `feae83dc9e9fb34c` | main | 0xc5ab8c | no (data) |  | queue pop: ldr x8, [x19] → dmb ishld (after the tail load; w8 = head) |
| 7 | `feae83dc9e9fb34c` | main | 0xc5ab94 | no (data) |  | queue pop: ldr w9, [x19, #0x1c] (the head again) → and x9, x8, #0xffff |
| 8 | `feae83dc9e9fb34c` | main | 0xc5ab98 | no (data) |  | queue pop: and x9, x9, #0xffff → ldr x8, [x19] (the ring; then ldr x8, [x8, x9, lsl #3]: the item) |

Bindings:

- **task ring push and pop barriers**: bound build (goal: game); a data race in the game's own task ring
- **UE3's GMalloc setup skipped, the host allocator in (hle_uemalloc.c)**: bound build (goal: game); UE3 inlines the allocator's constructor into the GMalloc setup and has its own vtable layout (25 slots, 32-bit sizes), so the Unreal 4/5 discovery does not apply

## Bayonetta (`010076F0049A2000`)

Profile keys: `pad_layout` full

## Bayonetta 2 (`01007960049A0000`)

Profile keys: `pad_layout` full

## Bayonetta 3 (`01004A4010FEA000`)

Profile keys: `pad_layout` full

## Big Brain Academy: Brain vs. Brain (`0100620012D6E000`)

Profile keys: `op_mode` handheld, `perf_mode` boost

## Bloodstained: Ritual of the Night (`0100BF500207C000`)

Profile keys: `fps_target` 60

## Borderlands: Game of the Year Edition (`010064800F66A000`)

Profile keys: `present_interval` display, `fps_target` 60

## Captain Toad: Treasure Tracker (`01009BF0072D4000`)

Profile keys: `op_mode` handheld, `pad_layout` full

## Catherine: Full Body (`0100BF00112C0000`)

### nxnative's patches (nxnative dm/games/0100BF00112C0000/patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| Catherine 60 fps startup and simulation | cuts | main |  |

Bindings:

- **Catherine 60 fps startup and simulation**: bound game (goal: game); GFD already accepts 60 at startup, but the title also selects its own fixed steps. Both sites must match together; no SDK or host setting selects the title’s timing mode.

## Cozy Land (`0100777022394000`)

Profile keys: `pad_layout` full

## DARK SOULS: REMASTERED (`01004AB00A260000`)

Profile keys: `pad_layout` full

### nxnative's patches (nxnative dm/games/01004AB00A260000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `df3766a2bb651a3e` | main | 0x706fa4 | no (data) |  | frame limiter target: mov w1,#30 → mov w1,#60 (main+0x707090 sleeps out 1000 / fps ms since the last frame) |
| 2 | `df3766a2bb651a3e` | main | 0x667904 | no (data) |  | frame update step: ldr s8,[x8,#0x6ec] (1/30) → ldr s8,[x8,#0x834] (1/60), passed to main+0x6a337c |

Bindings:

- **frame limiter 30 → 60 fps and the 1/60 update step**: bound build (goal: game); the game's own limiter constant and fixed step

## Diablo II: Resurrected (`0100726014352000`)

Profile keys: `present_interval` display

### nxnative's patches (nxnative dm/games/0100726014352000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `db0507f36b84200a` | main | 0x1979bc | no (data) |  | offline allowed (last online within 30 days): cset w8,lt → mov w8,#1 |
| 2 | `db0507f36b84200a` | main | 0x1979c4 | no (data) |  | offline allowed (no "User Last Online" setting): mov w8,wzr → mov w8,#1 |

| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| offline allowed without a recent Battle.net login | cuts | main | 0x13c |

Bindings:

- **offline play without a Battle.net login in the last 30 days**: bound game (goal: game); the game's own offline policy (a saved last-online time), which an offline host cannot satisfy; function-matched (MATCH_CUTS) over the game's updates

## Divinity: Original Sin 2 - Definitive Edition (`010027400CDC6000`)

Profile keys: `present_interval` display, `fps_target` 60

## Donkey Kong Bananza (`040074A01BF12000`)

Profile keys: `pad_layout` full

## DRAGON BALL: Sparking! ZERO (`010035F022078000`)

Profile keys: `fps_target` 60, `fps_max` 120, `pad_layout` full

## DRAGON QUEST BUILDERS 2 (`010042000A986000`)

Profile keys: `pad_layout` full

## DRAGON QUEST I & II HD-2D Remake (`0100D7C01F254000`)

Profile keys: `fps_target` 60, `pad_layout` full

## DRAGON QUEST III HD-2D Remake (`01003E601E324000`)

Profile keys: `fps_target` 60, `pad_layout` full

## DRAGON QUEST VII Reimagined (`0100A9D01C446000`)

Profile keys: `fps_target` 60, `ue_texture_pool` 1000, `pad_layout` full

## DRAGON QUEST XI S: Echoes of an Elusive Age – Definitive Edition (`01006C300E9F0000`)

Profile keys: `fps_target` 60, `pad_layout` full

## EA SPORTS FC 27 (`0100C49025D3E000`)

Profile keys: `shader_key_skip` 0x7d0+0x18, `fps_target` 60, `present_interval` display

## FINAL FANTASY RESONANCE (`0100606026F12000`)

Profile keys: `fps_target` 60, `pad_layout` full, `tex_dyn_slots` on

## FINAL FANTASY RESONANCE (Asian release) (`0100B8A026F14000`)

Profile keys: `fps_target` 60, `pad_layout` full, `tex_dyn_slots` on

## FINAL FANTASY TACTICS - The Ivalice Chronicles (`010038B015560000`)

Profile keys: `pad_layout` full, `tex_flush_upload` on, `tex_dirty_upload` on

### nxnative's patches (nxnative dm/games/010038B015560000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `3cfd457814dd647f` | main | 0x12554 | no (data) |  | job pool wake: ldr x0,[x0] → str x0,[x0,#8] (pending = nonzero: the woken worker signals once more) |
| 2 | `3cfd457814dd647f` | main | 0x12558 | no (data) |  | job pool wake: b SignalEvent → b 0x1254c (ldr x0,[x0]; b SignalEvent) |

Bindings:

- **job pool wake-up**: bound build (goal: game); a lost wake-up in the game's own job pool, not a host or SDK difference

## Fire Emblem Engage (`0100A6301214E000`)

Profile keys: `fps_target` 60

## Fire Emblem Fortune's Weave (`04007EF01F8AE000`)

Profile keys: `pad_layout` full, `fps_target` 60, `fps_max` 120

## Fire Emblem: Three Houses (`010055D009F78000`)

Profile keys: `pad_layout` full

### nxnative's patches (nxnative dm/games/010055D009F78000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `89048449ba238c8c` | main | 0xd13de0 | no (data) |  | per-render-mode present interval table {2,1,2,2,1,2} → all 1 (60 fps; the game logic already runs at 60 updates a second) |
| 2 | `89048449ba238c8c` | main | 0xd13e40 | no (data) |  | per-render-mode interval as float {2.0,1.0,2.0,2.0,1.0,2.0} → all 1.0 (render-thread per-frame steps) |
| 3 | `89048449ba238c8c` | main | 0x50fb7c | no (data) |  | render thread forced interval 2: orr w8,w9,w8 (either flag) → mov w8,wzr (the mode's interval, 1) |
| 4 | `89048449ba238c8c` | main | 0x50f8e8 | no (data) |  | render thread wait timeout: fmov s1,#1.0 → adrp x8,#0xcb7000 |
| 5 | `89048449ba238c8c` | main | 0x50f8ec | no (data) |  | render thread wait timeout: fdiv s0,s1,s0 → ldr s8,[x8,#0xb50] (1000.0) |
| 6 | `89048449ba238c8c` | main | 0x50f8f0 | no (data) |  | render thread wait timeout: adrp x8,#0xcb7000 → fdiv s0,s8,s0 (1000/fps; the fmul by s8 that follows makes it µs) |
| 7 | `89048449ba238c8c` | main | 0x50f8f4 | no (data) |  | render thread wait timeout: ldr s8,[x8,#0xb50] → nop (moved up) |

Bindings:

- **render intervals, the forced-interval flag and the render-wait timeout**: bound build (goal: game); the game's own tables, flag and timeout

## Ghost Trick: Phantom Detective (`010029B018432000`)

Profile keys: `tex_flush_upload` on, `tex_dirty_upload` on

## Hades (`0100535012974000`)

Profile keys: `pad_layout` full

## Hades II (`0100A00019DE0000`)

Profile keys: `pad_layout` full, `render_base` 1280x720 1280x720

### nxnative's patches (nxnative dm/games/0100A00019DE0000/patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| render size from the host's render scale | frame_step | main |  |

Bindings:

- **render size: 1280x720 times the host's render scale**: bound game (goal: game); the game's own size constant, found in the one function that calls nn::vi::CreateLayer on any build whose code there passes the stub's checks (no fingerprint); the game asks no service for a size, and no other game in the repository runs Supergiant's NVN engine (Hades is Vulkan)

## Hyrule Warriors: Age of Calamity (`01002B00111A2000`)

Profile keys: `pad_layout` full, `tex_flush_upload` on, `fps_target` 60, `fps_hold` off, `vconv_range` on

### nxnative's patches (nxnative dm/games/01002B00111A2000/patches.py)


| Stub | Kind | Build | Sites | Puts |
|---|---|---|---|---|
| 1 | frame_step | `c3cf52bf2b05d731` | 5 | KTGL device config present interval: 1 above a 30 fps cap, else the game's 2 (state page target fps); KIDS frame task: fps and the frame-rate block = the cap, vsyncs per update marked; device frame rate 59.94 / vsyncs (state page target fps; 0 = the game's 30); KIDS frame task: the frame-rate object's steps × its fps × vsyncs per update / 60 (state page target fps; 0 = as the game sets them); event cutscene's frame: stepped on every other update at a 60 fps cap (state page vsyncs per update; 30 fps frames, as on the Switch); event cutscene's frames left: stepped on every other update at a 60 fps cap (state page vsyncs per update; 30 fps frames, as on the Switch) |

| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| 60 fps frame step: the KIDS frame task, frame-rate block and event cutscene counters | frame_step | main | 0x1078 |

Bindings:

- **60 fps frame step: the KIDS frame task, frame-rate block and event cutscene counters (any build but 1.3.0)**: bound game (goal: game); the game's own frame code, found by its four functions' fingerprints, the KTGL device global and the frame-rate block read from their code. No KTGL sibling shares it: the KIDS task has no exact match in Three Hopes, Taiko Risshiden V DX, Three Houses, Hyrule Warriors: Definitive Edition or Age of Imprisonment
- **60 fps frame step on 1.3.0 (the same stub at its known addresses)**: bound build (goal: game); the stub above, placed at 1.3.0's addresses: the matched rule would serve it once its functions are fingerprinted Blocked: 1.3.0's main is on neither drive to fingerprint (v458752 is the update on them).

## Hyrule Warriors: Age of Imprisonment (`04003F40232DE000`)

Profile keys: `pad_layout` full

## Hyrule Warriors: Definitive Edition (`0100AE00096EA000`)

Profile keys: `pad_layout` full

## Kirby Air Riders (`040077201BE50000`)

Profile keys: `pad_layout` full, `tex_flush_upload` on, `tex_dirty_upload` on

## Kirby's Dream Buffet (`0100A8E016236000`)

### nxnative's patches (nxnative dm/games/0100A8E016236000/patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| BaseSystem initialization precedes Info setup | stubs | main | 0xf8 |

Bindings:

- **BaseSystem initialization before dependent Info setup**: bound game (goal: game); Scn.Step.Modules queues BaseSystem and Info setup together, but RewardGet.OnSetup caches the GamePauseManager that BaseSystem creates

## LEGO® Star Wars™: The Skywalker Saga (`010042D00D900000`)

Profile keys: `fps_target` 60, `present_interval` display

## Luigi's Mansion 3 (`0100DCA0064A6000`)

Profile keys: `fps_target` 60, `present_interval` display, `pad_layout` full

## Mario & Luigi: Brothership (`01006D0017F7A000`)

Profile keys: `fps_target` 60, `fps_max` 120, `pad_layout` full

## Mario + Rabbids Kingdom Battle (`010067300059A000`)

Profile keys: `pad_layout` full

## Mario + Rabbids® Sparks of Hope (`0100317013770000`)

Profile keys: `pad_layout` full

### nxnative's patches (nxnative dm/games/0100317013770000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `5b76a43b231e0640` | main | 0x94354 | no (data) |  | Unused 272-byte slot pool: 4,194,304 -> 65,536 slots, 1.06 GB -> 17 MB of address space (mov w1,#0x10000) |

Bindings:

- **unused 272-byte slot pool: 4,194,304 → 65,536 slots**: bound build (goal: game); Sparks of Hope's own init (Kingdom Battle has none)

## Mario Kart World (`0400C3F00006E000`)

Profile keys: `pad_layout` full, `fps_target` 60

### nxnative's patches (nxnative dm/games/0400C3F00006E000/patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| frame interval from the host's step | frame_step | main | 0x290 |
| frame interval from the host's step | frame_step | main | 0x28c |

Bindings:

- **frame interval from the host's step**: bound game (goal: game); the game's own interval choice, found by its function's fingerprint on any build that keeps the function

### A code patch table (`recomp.txt`)

Applied by nxrs builds of the module build it names.

## Mario Tennis Fever (`0400D4201C6EA000`)

Profile keys: `pad_layout` full

## Minecraft Dungeons II (`0100A7C01B792000`)

Profile keys: `tex_dyn_slots` on, `fps_target` 60, `fps_max` 120

## Monster Hunter Rise (`0100B04011742000`)

Profile keys: `pad_layout` full, `tex_flush_upload` on, `tex_dirty_upload` on, `fps_target` 60, `present_interval` display

## Monster Train 2 (`010051701FB46000`)

Profile keys: `pad_layout` full, `present_interval` display

## Need For Speed™ Hot Pursuit Remastered (`010029B0118E8000`)

Profile keys: `fps_target` 60, `present_interval` display

### nxnative's patches (nxnative dm/games/010029B0118E8000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `799d1061182c1302` | main | 0x4835a4 | no (data) |  | frame limiter spin: b.gt main+0x483588 → nop (the host paces the frames) |

Bindings:

- **frame limiter spin: its branch back cut**: bound build (goal: game); the engine's own spin limiter; a function-matched rule could carry it over the game's updates

## NieR:Automata The End of YoRHa Edition (`010056B015FE8000`)

Profile keys: `pad_layout` full, `present_interval` display, `fps_target` 60, `tex_flush_upload` on, `tex_dirty_upload` on

### nxnative's patches (nxnative dm/games/010056B015FE8000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `3c10ec35c5ef4c36` | main | 0x591198 | no (data) |  | 60 FPS present interval mode: 1 -> 0 |
| 2 | `3c10ec35c5ef4c36` | main | 0x9bad68 | no (data) |  | 60 FPS NVN present interval: 2 -> 1 |
| 3 | `3c10ec35c5ef4c36` | main | 0x9bf528 | no (data) |  | handheld lower bound follows preset minimum |
| 4 | `3c10ec35c5ef4c36` | main | 0x9bf534 | no (data) |  | docked lower bound follows preset minimum |
| 5 | `3c10ec35c5ef4c36` | main | 0x9bf434 | no (data) |  | dynamic resolution: bad frame cost 45 -> 22.5 ms at 60 fps |
| 6 | `3c10ec35c5ef4c36` | main | 0x9bf484 | no (data) |  | dynamic resolution: optimal frame cost 32.01 -> 16.005 ms at 60 fps (first load) |
| 7 | `3c10ec35c5ef4c36` | main | 0x9bf4d4 | no (data) |  | dynamic resolution: optimal frame cost 32.01 -> 16.005 ms at 60 fps (second load) |
| 8 | `3c10ec35c5ef4c36` | main | 0x9bf4f8 | no (data) |  | dynamic resolution: good frame cost 30 -> 15 ms at 60 fps |

Bindings:

- **60 fps intervals and dynamic-resolution constants**: bound build (goal: game); the game's own constants; the Asian release's function is the global one's 0x13c0 further on, so one function-matched rule could serve both releases. No PlatinumGames sibling shares the code: none of its sites has an exact match in ASTRAL CHAIN or Bayonetta 1-3 (nso_fnmap, 2026-11-09)

## NieR:Automata The End of YoRHa Edition (`0100B8E016F76000`)

Profile keys: `pad_layout` full, `present_interval` display, `fps_target` 60, `tex_flush_upload` on, `tex_dirty_upload` on

### nxnative's patches (nxnative dm/games/0100B8E016F76000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `98397a1041989465` | main | 0x9c07f4 | no (data) |  | dynamic resolution: bad frame cost 45 -> 22.5 ms at 60 fps |
| 2 | `98397a1041989465` | main | 0x9c0844 | no (data) |  | dynamic resolution: optimal frame cost 32.01 -> 16.005 ms at 60 fps (first load) |
| 3 | `98397a1041989465` | main | 0x9c0894 | no (data) |  | dynamic resolution: optimal frame cost 32.01 -> 16.005 ms at 60 fps (second load) |
| 4 | `98397a1041989465` | main | 0x9c08b8 | no (data) |  | dynamic resolution: good frame cost 30 -> 15 ms at 60 fps |
| 5 | `98397a1041989465` | main | 0x591198 | no (data) |  | NieR 1.0.3 frame interval default: 30 -> 60 fps (the mode the setter main+0x9c0590 stores at display +0x504) |
| 6 | `98397a1041989465` | main | 0x9bc128 | no (data) |  | NieR 1.0.3 NVN present interval default: 30 -> 60 fps (stored at the device's +0x9c160) |
| 7 | `98397a1041989465` | main | 0x9c08e8 | no (data) |  | dynamic resolution lower bound follows the selected preset |
| 8 | `98397a1041989465` | main | 0x9c08f4 | no (data) |  | dynamic resolution lower bound follows the selected preset |

Bindings:

- **60 fps intervals and dynamic-resolution constants**: bound build (goal: game); the game's own constants; the Asian release's function is the global one's 0x13c0 further on, so one function-matched rule could serve both releases. No PlatinumGames sibling shares the code: none of its sites has an exact match in ASTRAL CHAIN or Bayonetta 1-3 (nso_fnmap, 2026-11-09)

## OCTOPATH TRAVELER (`010057D006492000`)

Profile keys: `pad_layout` full, `fps_target` 60, `tex_flush_upload` on, `tex_dirty_upload` on

## OCTOPATH TRAVELER 0 (`01005270232F2000`)

Profile keys: `fps_target` 60, `tex_dyn_slots` on

## OCTOPATH TRAVELER II (`0100A3501946E000`)

Profile keys: `fps_target` 60, `pad_layout` full

## Orbitals (`040032B023CA4000`)

Profile keys: `pad_layout` full

## Ori and the Will of the Wisps (`01008DD013200000`)

### nxnative's patches (nxnative dm/games/01008DD013200000/patches.py)


| Stub | Kind | Build | Sites | Puts |
|---|---|---|---|---|
| 1 | stub | `11c33c7c2dbd3c54` | 0 |  |

Bindings:

- **Unity's constant-buffer table locked on the shared device**: bound build (goal: engine); Unity 2018.4's threaded GfxDevice: the main thread registers layouts on the real device while the render thread uses the same table; any Unity 2018.4 build with this code has the race Blocked: the stubs have no function-matched form yet, and no other Unity 2018.4 main on the drive shares these functions (Ori and the Blind Forest 1.0.2: no fingerprint match).

## Overcooked! All You Can Eat (`0100F28011892000`)

Profile keys: `fps_target` 60

## Paper Mario: The Thousand-Year Door (`0100ECD018EBE000`)

Profile keys: `pad_layout` full, `fps_target` 60, `fps_hold` off

### nxnative's patches (nxnative dm/games/0100ECD018EBE000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `0effe4af1dec3a79` | main | 0x2e77a4 | no (data) |  | frame step floor in 60 Hz vsyncs: mov w9,#2 → mov w9,#1 (the game steps by the vsyncs that passed, down to one) |

| Stub | Kind | Build | Sites | Puts |
|---|---|---|---|---|
| 1 | frame_step | `0effe4af1dec3a79` | 1 | frame step: the base clock steps by the host's vsyncs (state page target fps; 0 = the game's measured step) |

Bindings:

- **frame step floor 2 → 1 vsync, and its frame step**: bound build (goal: game); the game's own clock floor and step; MATCH_CUTS and MATCH_FRAME_STEP could carry the cut and the stub, both near the step's function (main+0x2e7750; one function not checked yet)

## Persona 4 Golden (`010062B01525C000`)

Profile keys: `pad_layout` full

## Persona 5 Royal (`01005CA01580E000`)

Profile keys: `pad_layout` full

## Persona 5 Strikers (`0100801011C3E000`)

Profile keys: `pad_layout` full

### nxnative's patches (nxnative dm/games/0100801011C3E000/patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| 60 Hz gameplay timing and presentation | cuts | main |  |

Bindings:

- **60 Hz gameplay timing and presentation**: bound game (goal: game); Strikers sets its own integer and float update rates, counter step and half-rate mode at startup and on scene changes. Changing the host or KTGL interval alone leaves those writers at 30 Hz.

## Persona 5 Tactica (`010087701B092000`)

Profile keys: `pad_layout` full, `stdalloc_cap` 0x60000000, `fps_target` 60

## Pikmin 4 (`0100B7C00933A000`)

Profile keys: `fps_target` 60, `fps_max` 120, `pad_layout` full

## Pokemon Pokopia (`0400A3A01EE4C000`)

Profile keys: `pad_layout` full

## Pokémon Brilliant Diamond (`0100000011D90000`)

Profile keys: `fps_target` 60, `present_interval` display

## Pokémon Legends: Arceus (`01001F5010DFA000`)

Profile keys: `pad_layout` full

### nxnative's patches (nxnative dm/games/01001F5010DFA000/patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| capture demo: the empty result when the weak reference at +0x88 is unset or dead | cuts | main | 0xa70 |

Bindings:

- **capture demo: the empty result when the weak reference at +0x88 is unset or dead**: bound game (goal: game); the game's own function dereferences its own weak reference; no host service is on the path, so no host or engine rule can hold it. Whether another Game Freak game shares the function is unchecked

## Pokémon Legends: Z-A (`0100F43008C44000`)

Profile keys: `pad_layout` simple

### the owner's patches (Turbo) (Turbo nxloader.py (the owner's built-in patches))

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `b1f12fd919eae86a` | main | 0x1b296d8 | yes | `NXRS_PATCH_ZA_60FPS=0` | Pokemon Legends Z-A: base frame rate 60 instead of 30 (ldrsb w20, [x9, #0x20] -> mov w20, wzr). Update 2.0.2: once a frame main+0x1B29660 reads the frame rate the game runs at from its frame-rate settings (the byte at +0x20 of the object at +0x40: 0 FPS60, 1 FPS30, 2 FPS20, 3 FPS15, 4 FPS12, trinity_config's baseFrameRate) and hands it to the window (present interval = value + 1, main+0x17FB5E0). The package's trinity_config.trowc does not set it and the program's own value is FPS30. With FPS60 read there the game steps by measured time as before (falling back to 30, 20... by its own rule when frames take longer) and held 58 to 60 in Turbo. |

## Pokémon Scarlet (`0100A3D008C5C000`)

Profile keys: `pad_layout` full

## Pokémon Shield (`01008DB008C2C000`)

Profile keys: `pad_layout` full

## Pokémon Sword (`0100ABF008968000`)

Profile keys: `pad_layout` full

## Pokémon Violet (`01008F6008C5E000`)

Profile keys: `pad_layout` full

## Portal (`01007BB017812000`)

Profile keys: `pad_layout` full

## Portal 2 (`0100ABD01785C000`)

Profile keys: `pad_layout` full

## Princess Peach: Showtime! (`01007A3009184000`)

Profile keys: `fps_target` 60

## Red Dead Redemption (`01007820196A6000`)

Profile keys: `fps_target` 60, `present_interval` display

## Resident Evil (`010050F00BC1A000`)

Profile keys: `tex_flush_upload` on, `tex_dirty_upload` on

## Return of the Obra Dinn (`010032E00E6E2000`)

Profile keys: `pad_layout` full

## Rhythm Heaven Groove (`0100D9F01D474000`)

Profile keys: `op_mode` handheld, `perf_mode` boost

## ROMANCE OF THE THREE KINGDOMS 8 REMAKE (`0100C2A01A730000`)

Profile keys: `pad_layout` full

## Sea of Stars (`01008C0016544000`)

Profile keys: `pad_layout` full

## SHIN MEGAMI TENSEI V: Vengeance (`010069C01AB82000`)

Profile keys: `fps_target` 60, `ue_fps_cvar` Project.FrameRateLimit, `pad_layout` full

## Sonic Superstars (`01008F701C074000`)

Profile keys: `stdalloc_cap` 0x60000000

## Splatoon Raiders (`0400380020056000`)

Profile keys: `pad_layout` full

## Star Fox (`0400E88025B1A000`)

Profile keys: `pad_layout` full

## Stardew Valley (`0100E65002BB8000`)

Profile keys: `pad_layout` full

## Subnautica (`0100429011144000`)

Profile keys: `pad_layout` full

## Subnautica: Below Zero (`010014C011146000`)

Profile keys: `pad_layout` full

## Super Mario 3D All-Stars: Super Mario 64 (`010049900F546001`)

Profile keys: `pad_layout` full, `launch_param` lang8 f32:1.12 f32:2.24

## Super Mario 3D All-Stars: Super Mario Sunshine (`010049900F546002`)

Profile keys: `pad_layout` full, `op_mode` handheld, `launch_param` lang8 f32:0.5 f32:1

### nxnative's patches (nxnative dm/games/010049900F546002/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `b21e3733e11d6fcf` | main | 0x151214 | no (data) |  | coroutine stack region size: orr w2,wzr,#0x40000000 → mov w2,#0x4000000 (1 GB → 64 MB) |
| 2 | `b21e3733e11d6fcf` | main | 0x151398 | no (data) |  | coroutine stack region size, the other start path: 1 GB → 64 MB |

Bindings:

- **Hagi coroutine stack region 1 GB → 64 MB**: bound build (goal: game); a size inside Hagi's own code: nothing the host sees tells a 1 GB reservation it can shrink from one it cannot

## Super Mario 3D World + Bowser's Fury (`010028600EBDA000`)

Profile keys: `pad_layout` full

## Super Mario Galaxy (`010099C022B96000`)

Profile keys: `pad_layout` full, `op_mode` handheld

## Super Mario Galaxy 2 (`0100FD8022DAA000`)

Profile keys: `pad_layout` full, `op_mode` handheld

## Super Mario Odyssey (`0100000000010000`)

Profile keys: `pad_layout` full

## Super Smash Bros. Ultimate (`01006A800016E000`)

### nxnative's patches (nxnative dm/games/01006A800016E000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `21450c647b8c5940` | main | 0x37c9a5c | no (data) |  | AllocateAddressRegion size mov x1,#0xf80000000 → mov x1,#0x100000000 (62 GB → 4 GB) |
| 2 | `21450c647b8c5940` | main | 0x37c9a6c | no (data) |  | node pool top check, low part mov x9,#0xffffffff0 → mov x9,#0xfff0 |
| 3 | `21450c647b8c5940` | main | 0x37c9a70 | no (data) |  | node pool start mov x11,#0x7f200000 → mov x11,#0xff200000 |
| 4 | `21450c647b8c5940` | main | 0x37c9a74 | no (data) |  | node pool top check movk x9,#0x7f20,lsl #16 → movk x9,#0xff20,lsl #16 (0xff20fff0) |
| 5 | `21450c647b8c5940` | main | 0x37c9a78 | no (data) |  | node pool start movk x11,#0xf,lsl #32 → movk x11,#0,lsl #32 (0xff200000 = 4 GB − 14 MB) |
| 6 | `21450c647b8c5940` | main | 0x37c9a9c | no (data) |  | node pool bound: region end mov x9,#0xf80000000 → mov x9,#0x100000000 |
| 7 | `21450c647b8c5940` | main | 0x37c9b98 | no (data) |  | first free range in 64 KB units mov w1,#0x7f20 → mov w1,#0xff20 |
| 8 | `21450c647b8c5940` | main | 0x37c9b9c | no (data) |  | first free range movk w1,#0xf,lsl #16 → movk w1,#0,lsl #16 (0xff20 units = 4 GB − 14 MB) |
| 9 | `21450c647b8c5940` | main | 0x37c9c50 | no (data) |  | node pool bound: region end mov x9,#0xf80000000 → mov x9,#0x100000000 |
| 10 | `21450c647b8c5940` | main | 0x37ca3cc | no (data) |  | node pool bound: region end mov x8,#0xf80000000 → mov x8,#0x100000000 |
| 11 | `21450c647b8c5940` | main | 0x37caf68 | no (data) |  | node pool bound: region end mov x9,#0xf80000000 → mov x9,#0x100000000 |

| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| engine region 62 GB → 4 GB (11 instructions in four functions) | cuts | main |  |

Bindings:

- **engine region 62 GB → 4 GB (11 cuts in four functions)**: bound game (goal: game); function-matched for builds of the same code (MATCH_CUTS); the region and its node pool are the nu:: engine's own

## Taiko Risshiden V DX (`0100346017304000`)

Profile keys: `op_mode` handheld, `pad_layout` touch

### nxnative's patches (nxnative dm/games/0100346017304000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `f5ab74b9d45f955d` | main | 0x443fac | no (data) |  | frame limiter threshold: N × tick frequency / 60 (umulh by 1/60 → udiv by 60), then 0 when N = 1 (cmp w20, #1; csel x20, xzr, x8, eq): one vsync a frame is the present pacing's; N = 2 keeps the 2 ms rounds |

| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| frame limiter threshold: 0 when N = 1 | cuts | main | 0x16c |
| frame limiter threshold: 0 when N = 1 | cuts | main | 0xac |

Bindings:

- **KTGL frame limiter threshold: 0 at one vsync a frame**: bound game (goal: game); function-matched over its two builds (MATCH_CUTS); whether Age of Calamity, Three Houses or ROMANCE OF THE THREE KINGDOMS 8 REMAKE share the function is unchecked (an engine rule if they do)

## The Binding of Isaac: Rebirth (`01009BC00C38A000`)

Profile keys: `pad_layout` full

## The Great Ace Attorney Chronicles (`010036E00FB20000`)

Profile keys: `tex_flush_upload` on, `tex_dirty_upload` on

## The Legend of Zelda: Breath of the Wild (`01007EF00011E000`)

Profile keys: `render_base` 1600x900 1280x720, `pad_layout` full, `fps_target` 60, `screen_aspect` on

### nxnative's patches (nxnative dm/games/01007EF00011E000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `cd57b23fa4bbad65` | main | 0x112670c | no (data) |  | lower bound of the candidate interval: fmov s0,s11 (2.0) → fmov s0,s10 (1.0) |
| 2 | `cd57b23fa4bbad65` | main | 0x1126710 | no (data) |  | lower bound of the candidate interval: cmp w27,#2 → cmp w27,#1 |
| 3 | `cd57b23fa4bbad65` | main | 0x112665c | no (data) |  | step-down target recorded when stepping up, max(candidate, w24) − 1: mov w24,#3 → mov w24,#2 |
| 4 | `cd57b23fa4bbad65` | main | 0x1126748 | no (data) |  | step-down target recorded when stepping up: cmp w8,#3 → cmp w8,#2 |
| 5 | `cd57b23fa4bbad65` | main | 0x1126794 | no (data) |  | stepping down stops at < 3 → at < 2: cmp w9,#3 → cmp w9,#2 |
| 6 | `cd57b23fa4bbad65` | main | 0x112691c | no (data) |  | interval controller always takes the GPU time path (CPU span timing): cbz x0,#0x1126934 → b #0x1126928 |
| 7 | `cd57b23fa4bbad65` | main | 0x1126974 | no (data) |  | interval controller always takes the GPU time path (takes max(CPU span, GPU time)): cbz x0,#0x112698c → b #0x1126980 |

| Stub | Kind | Build | Sites | Puts |
|---|---|---|---|---|
| 1 | frame_step | `cd57b23fa4bbad65` | 44 | setWaitVsyncInterval: the host's step unless the VFR override is set (state page target fps; 0 = the game's interval); VFR::updateInterval: its own override published for the host (state page target fps; 0 = original); held-button counts step by the VFR ratio (menu repeat in game time at any frame step); 3D view scale = the host render scale (1.0 = the game's dynamic resolution); scale apply block taken every frame when the host render scale is not 1.0; output size static, nnMain: 1920x1080 → the host's height; output size static, nnMain (second path); output size static, its getter; layout size static, nnMain: 1280x720 → the host's layout height; layout size static, its getter; docked display size 1920x1080; docked viewport height 1080; handheld display size 1280x720; handheld viewport height 720; scaled display height 720.0; 720.0: output size / layout size (the layout scale); 720.0 with 1280.0 (screen position in layout space); 720.0 with 1280.0 (screen position / layout size); 720.0 with 1280.0; 720.0 stored with 1280.0; 720.0 with 1280.0 (screen position); 720.0 with 1280.0 passed as a layout size; 720.0 with 1280.0; 1280x720 layout size of an object; 1280x720 default sizes of a layer; 1280x720 passed as a size; 1280x720 in a descriptor; sead::PerspectiveProjection fovy widened for a view of the screen's ratio (more above and below, 16:9 sides), stored widened; sead::PerspectiveProjection fovy widened for a view of the screen's ratio (more above and below, 16:9 sides), stored widened; sead::PerspectiveProjection fovy widened for a view of the screen's ratio (more above and below, 16:9 sides), stored widened; main camera fovy getter: widened like the projection (culling tests); main camera fovy getter in degrees: widened like the projection; PerspectiveProjection matrix: the HUD camera (16:9, near 1, far 10000) takes the screen's ratio and a widened vertical angle; Pane::Calculate: a HUD widget at the edge moved to the taller screen's edge with its children, full-screen backgrounds stretched (duo_anchor.c); capture: the screen's layout height instead of the layout file's 720 (texture size, copied rect); capture: the screen's layout height instead of the layout file's 720 (texture size, copied rect); capture: the screen's layout height instead of the layout file's 720 (texture size, copied rect); capture: the screen's layout height instead of the layout file's 720 (texture size, copied rect); capture pane Calculate: a full-screen capture made as tall as the screen (duo_anchor.c); capture pane Calculate: a full-screen capture made as tall as the screen (duo_anchor.c); map footprint trail: its viewport and projection at the 720 pane's half-height, not the taller frame's; projected texture fitted to the layout: a pane as tall as the taller screen maps its whole height; screen copy into a 16:9 picture (camera photo): the screen's middle 16:9 band, not the whole taller screen squashed into it |

Bindings:

- **interval controller: the lower bound and the GPU-time path**: bound build (goal: game); sead::GameFrameworkNx's interval controller in this build; Tears of the Kingdom shares sead but not this code
- **60 fps frame step and render scale (ksys::VFR)**: bound build (goal: game); the game's own frame code; the frame-step state page is general. No engine sibling shares it: none of its 34 functions has an exact match in Tears of the Kingdom, SMO, MK8D, ACNH, Splatoon 2 / 3, Kirby, Mario Party, Ocarina, Smash, Metroid Dread or Origami King (only the widescreen stubs' sead::PerspectiveProjection functions recur)

## The Legend of Zelda: Echoes of Wisdom (`01008CF01BAAC000`)

Profile keys: `pad_layout` full

## The Legend of Zelda: Link's Awakening (`01006BB00C6F0000`)

### nxnative's patches (nxnative dm/games/01006BB00C6F0000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `909e904af78ac1b8` | main | 0x10cd070 | no (data) |  | physics completion task release (calls its own destructor): ldr x8,[x0] → ret (no longer destroyed; the step function rebuilds it in place every step) |

Bindings:

- **PhysX completion task release**: bound build (goal: game); a lifetime race in the game's own completion task (its release slot serves this one vtable)

## The Legend of Zelda: Ocarina of Time (`0400486024DEC000`)

Profile keys: `pad_layout` full, `fps_target` 60, `draw_barriers` game, `tex_flush_upload` on, `tex_dirty_upload` on

### nxnative's patches (nxnative dm/games/0400486024DEC000/patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| movie pictures a frame: up to 2, the constructor's value for an argument of 2 or more, whatever its caller passes (1) | cuts | main | 0x33c |
| movie update cap: 3 pictures a frame, the frame step's largest hold, in place of the limit (2), which also sizes the decoder pipeline | cuts | main | 0x170 |
| movie frame: the pictures the player has taken (its +0xc), not 1 more for each frame drawn | cuts | main |  |
| frame ratio from the host's step | frame_step | main | 0x9b4 |

Bindings:

- **movie pictures a frame: up to 2**: bound game (goal: game); the game's own movie player constructor, found by its function's fingerprint on any build that keeps the function; no other game has it: the fingerprint matches no function in the mains of Donkey Kong Bananza, Splatoon Raiders, Pokopia, Mario Tennis Fever, Switch Sports, Yoshi and the Mysterious Book, Mario Golf, Xenoblade Chronicles 2 or Super Mario 3D All-Stars, and Breath of the Wild's and Tears of the Kingdom's movies go through subsdk0's movie::Decoder, not a player in main
- **movie update cap: 3 pictures a frame**: bound game (goal: game); the game's own movie player update, found by its function's fingerprint on any build that keeps the function; it goes with the player's two-picture cut and has no wider binding
- **movie frame: the pictures the player has taken**: bound game (goal: game); the game's own movie draw functions, found by their functions' fingerprints on any build that keeps them; it is the counter the same game's player keeps, so it goes with the player's two-picture cut and has no wider binding
- **frame ratio from the host's step**: bound game (goal: game); the game's own frame-ratio division, found by its function's fingerprint on any build that keeps the function
- **dynamic resolution: full size, or a climb on the load measured at each level**: bound game (goal: game); the game's own dynamic-resolution level function and update, found by their functions' fingerprints on any build that keeps both; switchable (DM_DYNAMIC_RESOLUTION)

### the owner's patches (Turbo) (Turbo nxloader.py (the owner's built-in patches))

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `e0f3ccfaf50065ae` | main | 0x805bd0 | yes | `NXRS_PATCH_OOT_FULLRES=0` | Ocarina of Time: full resolution (tbz w8, #0 -> b). Take the "dynamic resolution disabled" path so the 3D renders at full scale instead of following its GPU time controller down to 480p (from Lichii's built-in patch list). The word at the offset has to be the original or nothing is changed. |

## The Legend of Zelda: Tears of the Kingdom (`0100F2C0115B6000`)

Profile keys: `pad_layout` full, `tex_flush_upload` on, `tex_dirty_upload` on, `fps_target` 60

### nxnative's patches (nxnative dm/games/0100F2C0115B6000/patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| 60 fps frame step and render scale | frame_step | main | 0x298 |

Bindings:

- **60 fps frame step and render scale**: bound game (goal: game); the game's own timing object, found by its six functions' fingerprints on any build that keeps them; no engine sibling shares the code (BotW, SMO, MK8D, ACNH, Splatoon 2 / 3, Kirby, Mario Party, Ocarina, Smash, Metroid Dread, Origami King: no exact match, 2026-11-09)

## The Rogue Prince of Persia (`01008D9022462000`)

Profile keys: `fps_target` 60, `fps_max` 120

## The Witcher 3: Wild Hunt — Complete Edition (`0100A0800E9C4000`)

Profile keys: `fps_target` 60, `present_interval` display

## TRIANGLE STRATEGY (`0100CC80140F8000`)

Profile keys: `fps_target` 60, `pad_layout` full

## TUNIC (`0100DA801624E000`)

Profile keys: `fps_target` 60

## Unicorn Overlord (`010069401ADB8000`)

Profile keys: `alloc_big_align` on

## Unicorn Overlord (Asia) (`010054B01AD92000`)

Profile keys: `alloc_big_align` on

## Xenoblade Chronicles 2 (`0100E95004038000`)

Profile keys: `pad_layout` full

## Xenoblade Chronicles 3 (`010074F013262000`)

Profile keys: `pad_layout` full

### nxnative's patches (nxnative dm/games/010074F013262000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `82d187fe9ef9be92` | main | 0x128e58c | no (data) |  | malloc address region: 60 GB to 4 GB |
| 2 | `82d187fe9ef9be92` | main | 0x1241870 | no (data) |  | first arena bump size, low part: mov x9,#0xffff |
| 3 | `82d187fe9ef9be92` | main | 0x1241880 | no (data) |  | first arena bump size: 512 MB + 128 KB + 64 KB - 1 |
| 4 | `82d187fe9ef9be92` | main | 0x12418d8 | no (data) |  | first arena subtraction, sign extension |
| 5 | `82d187fe9ef9be92` | main | 0x12418dc | no (data) |  | first arena subtraction: -(512 MB + 128 KB) |
| 6 | `82d187fe9ef9be92` | main | 0x12418e8 | no (data) |  | first arena stored capacity: 512 MB + 128 KB |

Bindings:

- **malloc region 60 GB → 4 GB and its first arena**: bound build (goal: game); Monolith's own allocator; its constants span two functions (a multi-function rule). The cause, an iPhone's address space, is a platform limit

## Xenoblade Chronicles X: Definitive Edition (`0100453019AA8000`)

Profile keys: `pad_layout` full, `fps_target` 60

### nxnative's patches (nxnative dm/games/0100453019AA8000/patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `3f2425864cf22684` | main | 0x13f3cf0 | no (data) |  | malloc address region: 60 GB to 4 GB |
| 2 | `3f2425864cf22684` | main | 0xbabe70 | no (data) |  | first arena bump size, low part: mov x9,#0xffff |
| 3 | `3f2425864cf22684` | main | 0xbabe80 | no (data) |  | first arena bump size: 512 MB + 128 KB + 64 KB - 1 |
| 4 | `3f2425864cf22684` | main | 0xbabf04 | no (data) |  | first arena subtraction, sign extension |
| 5 | `3f2425864cf22684` | main | 0xbabf0c | no (data) |  | first arena subtraction: -(512 MB + 128 KB) |
| 6 | `3f2425864cf22684` | main | 0xbabf1c | no (data) |  | first arena stored capacity: 512 MB + 128 KB |

| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| Monolith malloc region and first arena | cuts | main |  |
| camera angular integration uses elapsed time | stubs | main | 0xadc |

Bindings:

- **malloc region 60 GB → 4 GB and its first arena**: bound game (goal: game); Monolith's allocator calls nn::os directly; its sizes span two functions matched atomically. The 512 MB first arena is sized for this game; an engine rule needs shared-layout proof and dynamic sizing
- **camera angular integration uses elapsed time**: bound game (goal: game); the game's orbit camera integrates configured angular speed once per update; multiply the loaded gain by its own 30 Hz elapsed-time ratio. Engine input is amplitude state and must not be scaled

## Xenoblade Chronicles: Definitive Edition (`0100FF500E34A000`)

Profile keys: `pad_layout` full

## Yoshi and the Mysterious Book (`0400245021D2C000`)

Profile keys: `pad_layout` full

## Yoshi's Crafted World (`01006000040C2000`)

Profile keys: `tex_flush_upload` on, `tex_dirty_upload` on

## ŌKAMI HD (`0100276009872000`)

Profile keys: `pad_layout` full

## Shared patch sets

### shared (nxnative dm/tools/sdk_patches.py)

| Cut | Build | Module | Offset | Applied by nxrs | Off switch | Reason |
|---|---|---|---|---|---|---|
| 1 | `9de92bad67c41502` | main | 0x93dfc | no (data) |  | VammManager node area limit: 1 GB to 64 MB |
| 2 | `9de92bad67c41502` | main | 0x93e8c | no (data) |  | VammManager occupied node area: 1 GB to 64 MB |
| 3 | `9de92bad67c41502` | main | 0x93fe0 | no (data) |  | AllocateAddressRegion: align regions of 1 GB and more to 32 MB |

| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0x114 |
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0x110 |
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0x110 |
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0xf0 |
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0xf4 |
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0x114 |
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0x148 |
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0x12c |
| AddressRegionManager node area 1 GB → 62 MB | cuts | sdk | 0x14c |
| AllocateAddressRegion: regions of 4 MB and more aligned to 2 MB, as SDK 20 | cuts | sdk | 0x160 |
| AllocateAddressRegion: regions of 4 MB and more aligned to 2 MB, as SDK 20 | cuts | sdk | 0x168 |
| AllocateAddressRegion: regions of 4 MB and more aligned to 2 MB, as SDK 20 | cuts | sdk | 0x160 |
| AllocateAddressRegion: regions of 4 MB and more aligned to 2 MB, as SDK 20 | cuts | sdk | 0x184 |

Bindings:

- **node area 1 GB → 62 MB and regions of 4 MB and more aligned to 2 MB, as SDK 20: the SDK-wide rules**: bound sdk (goal: sdk); one rule per reviewed implementation; every sdk build without CUTS gets them
- **VammManager node area and alignment, SDK 4.4.0 (9de92bad)**: bound build (goal: sdk); an older implementation no rule covers (its large regions aligned to 32 MB, not the rules' 2 MB); a rule once a second game ships it Blocked: one game ships it.

### shared (nxnative dm/tools/engine_patches.py)


| Matched rule | Kind | Module | Function size |
|---|---|---|---|
| Unity 2018 ThreadedStreamBuffer: load-acquires after its store-releases | cuts | main |  |

(125 games listed.)
