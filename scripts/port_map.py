#!/usr/bin/env python3
"""PORT_MAP.tsv: every file of the trees nxrs ports from, and where it went (plan section 4).

    py -I scripts/port_map.py [--nx <nxnative>] [--turbo <Switch2RECOMP-main>] [--mr <mkw-recomp-main>]   write
    py -I scripts/port_map.py --check [...]                                                                 verify
    py -I scripts/port_map.py --check --format-only                                                         rows only (CI)

Trees (tree column):
  NX  nxnative (every tracked file)                    default build/Q/nxnative-up, else D:/Projects/nxnative
  TB  Turbo, the owner's engine (branch turbo, tracked) default D:/Projects/Switch2RECOMP-main
  MR  mkw-recomp, only the donor files of section 4.7  default D:/Projects/mkw-recomp-main

Columns: tree, path, lines (wc -l), dest (crate/module, or - ), action, status, note.
  action: PORT, REWRITE, DONOR, DATA, TOOL, REF, DROP, DEFER  (plan section 4)
  status: Done, Partial, Not started, Deferred, Dropped, Reference

--check: with a tree present, regenerates its rows and compares (every file has exactly one row, line counts equal);
with a tree absent (CI), checks the rows that are there are well formed. Seconds either way.
"""
import os
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "PORT_MAP.tsv"
ACTIONS = {"PORT", "REWRITE", "DONOR", "DATA", "TOOL", "REF", "DROP", "DEFER"}
STATUSES = {"Done", "Partial", "Not started", "Deferred", "Dropped", "Reference"}

# Rules: first match wins. (regex over the tree path, dest, action, status, note)
NX = [
    # ---- dm/host
    (r"dm/host/(Makefile|\.gitignore)$", "-", "DROP", "Dropped", "Cargo"),
    (r"dm/host/dm\.h$", "os::log, config", "PORT", "Done", "logging and DM_* switches"),
    (r"dm/host/registration\.h$", "os::hle", "REWRITE", "Done", "registry tables"),
    (r"dm/host/hle\.[ch]$", "os::hle, runtime::hostfn", "PORT", "Done", "host functions at guest addresses (3.2)"),
    (r"dm/host/(entry\.S|thread\.c)$", "runtime::{rt,svc,hostfn}", "DROP", "Dropped", "replaced by the mkw-recomp bridge (DONOR)"),
    (r"dm/host/hle_bigstack\.c$", "-", "DROP", "Dropped", "host code runs on host stacks"),
    (r"dm/host/hle_svc\.c$", "os::svc, os::mem, runtime::mem", "PORT", "Partial", "memory SVCs on the Windows mechanism; SetMemoryPermission/Attribute, resource limits, GetSystemInfo 0x6F answer success without the work (AUDIT)"),
    (r"dm/host/hle_thread\.c$", "os::sched, runtime::kernel", "PORT", "Partial", "Win32 threads, WaitOnAddress; QoS / MMCSS classes (DM_QOS_MAP) missing (AUDIT)"),
    (r"dm/host/hle_guestthread\.c$", "os::guest", "PORT", "Done", ""),
    (r"dm/host/hle_os\.c$", "os::services::os, os::clock", "PORT", "Partial", "DM_DETCLOCK (clock steps per present) missing"),
    (r"dm/host/hle_table\.c$", "os::services::simple_table", "REWRITE", "Done", "libc shims"),
    (r"dm/host/hle_time\.c$", "os::services::time", "PORT", "Done", ""),
    (r"dm/host/hle_sf\.c$", "os::services::sf, os::ipc", "PORT", "Done", ""),
    (r"dm/host/hle_sdk(init|16)\.c$", "os::services::sdkinit", "PORT", "Done", ""),
    (r"dm/host/hle_oe\.c$", "os::services::oe", "PORT", "Partial", "focus change / message 15 never sent (AUDIT)"),
    (r"dm/host/hle_lifecycle\.c$", "os::services::oe", "PORT", "Partial", "console control handler: front's quit path"),
    (r"dm/host/(hle_account(_users)?\.c|account_users\.h)$", "os::services::account", "PORT", "Partial", "ShowUserCreator answers ok without a creator"),
    (r"dm/host/user_select_mac\.h$", "front", "REWRITE", "Not started", "user picker"),
    (r"dm/host/hle_fs\.c$", "os::services::fs", "PORT", "Partial", "RomFS in place; SD card, BCAT save, save-data info reader missing; save\\user<N> differs from Turbo's save\\user (AUDIT)"),
    (r"dm/host/romfs_(image|identity)\.h$", "-", "DROP", "Dropped", "iOS RomFS image; read in place"),
    (r"dm/host/save_sync\.[ch]$", "-", "DROP", "Dropped", "iCloud"),
    (r"dm/host/(hle_hid\.c|dm_input\.h)$", "os::services::hid, os::input", "PORT", "Partial", "Palma 4 of 24; DM_PAD_* / DM_RUMBLE settings missing"),
    (r"dm/host/hle_touch\.c$", "os::services::hid", "PORT", "Not started", "touch_fill is always idle (AUDIT: stub)"),
    (r"dm/host/hle_haptics\.[ch]$", "os::services::hid_ext, os::input", "PORT", "Partial", "rumble"),
    (r"dm/host/hle_irs\.c$", "os::services::hid_ext", "PORT", "Partial", ""),
    (r"dm/host/(haptics_apple\.[hm]|motion_apple\.h|motion_pro_mac\.h|pad_hid_mac\.h|pad_jc1_mac\.h|pad_switch2_apple\.[hm])$",
     "os::input::{hidio,nintendo,switch2_ble,sony,xinput}", "REWRITE", "Done", "hidapi / WinRT BLE (DONOR switch2_win.c)"),
    (r"dm/host/motion_fusion\.h$", "os::input::motion", "PORT", "Done", ""),
    (r"dm/host/(motion_pro|pad_apple|pad_slots|pad_hid|pad_pro|pad_jc1)\.h$", "os::input", "PORT", "Done", "pure parsers"),
    (r"dm/host/switch2_proto\.h$", "os::input::switch2", "PORT", "Done", ""),
    (r"dm/host/(ctrl_prompt|home_quit|home_quit_mac|front|front_role)\.h$", "front", "PORT", "Partial", ""),
    (r"dm/host/front_mac\.m$", "front::{window,keys,help}", "REWRITE", "Partial", "Win32 window; cheats panel not done"),
    (r"dm/host/memstat\.m$", "config::metrics", "REWRITE", "Done", "GetProcessMemoryInfo"),
    (r"dm/host/(crash|probe)\.c$", "runtime::fault", "REWRITE", "Partial", "VEH, host PC -> guest function; no minidump"),
    (r"dm/host/loader\.c$", "runtime::loader, os::boot", "PORT", "Done", ""),
    (r"dm/host/main\.c$", "os::boot, front", "PORT", "Done", "nn::init::Start order"),
    (r"dm/host/hle_audio\.c$", "os::services::audio, os::media::sink", "PORT", "Partial", "sink on WASAPI; device choice, mute, WAV dump missing"),
    (r"dm/host/hle_audren\.c$", "os::services::audren", "PORT", "Done", ""),
    (r"dm/host/hle_audren_sv\.c$", "os::services::audren", "PORT", "Not started", "audren:u for SDK 1.x (no owner game)"),
    (r"dm/host/audio_input(_mac)?\.[chm]$", "os::media::mic", "REWRITE", "Partial", ""),
    (r"dm/host/hle_codec\.c$", "os::services::codec, os::media::opus", "PORT", "Done", ""),
    (r"dm/host/(hle_codec_video\.c|hle_movie\.c|hle_movie_av1\.[ch]|hle_movie_yuv\.h|hle_nvdrv\.c)$",
     "os::services::{codec_video,movie,nvdrv}, os::media", "PORT", "Partial", "InitializeH264Decoder overload, Extractor::SetDataSource missing"),
    (r"dm/host/(hle_movie_avc\.[ch]|hle_movie_vt\.h)$", "os::media::mf", "REWRITE", "Done", "Media Foundation"),
    (r"dm/host/hle_cheats\.c$", "os (cheats)", "PORT", "Not started", "code cheats need a rebuild as image patches (R2)"),
    (r"dm/host/hle_fpslock\.c$", "os::gamefix, profiles", "PORT", "Not started", "engine frame lock missing; profile fps_target unread at run time (AUDIT)"),
    (r"dm/host/hle_jit\.c$", "-", "DEFER", "Deferred", "needs run-time translation"),
    (r"dm/host/hle_ro\.c$", "os::services::ro", "PORT", "Done", "NROs translated at build time"),
    (r"dm/host/hle_(mii|misc|friends)\.c$", "os::services::{mii,misc,simple}", "PORT", "Done", ""),
    (r"dm/host/hle_swkbd\.c$", "os::services::swkbd", "PORT", "Partial", "10 InlineKeyboard methods not taken over"),
    (r"dm/host/hle_offline\.c$", "os::services::offline", "PORT", "Partial", "PlayReport::Send, ngc, album URL missing; web applet closes at once"),
    (r"dm/host/hle_net\.[ch]$", "os::services::{net,socket}", "PORT", "Partial", "ssl GetBuiltInCertificates missing"),
    (r"dm/host/(hle_web\.c|web_apple\.m)$", "-", "DEFER", "Deferred", "WebView2 later"),
    (r"dm/host/hle_(oodle|glslc|uecvar|uemalloc)\.c$", "os", "DEFER", "Deferred", "no owner game needs it yet"),
    (r"dm/host/hle_count\.c$", "config::metrics", "PORT", "Partial", "counter registry exists; no producer in any crate (AUDIT)"),
    (r"dm/host/hle_fdstat\.c$", "config::metrics", "PORT", "Not started", "handle / fd statistics"),
    (r"dm/host/tests/.*_(mac|apple)\.m$", "-", "DROP", "Dropped", "Apple-only tests"),
    (r"dm/host/tests/", "crate tests (os, runtime)", "PORT", "Partial", "logic tests re-expressed where the module is ported"),
    # ---- dm/gfx
    (r"dm/gfx/(README\.md|nvn_metal_mapping\.md)$", "graphics (docs)", "REWRITE", "Partial", "INTERFACES graphics"),
    (r"dm/gfx/hle_(nv|vi)\.c$", "os::nvn::nv, os::services::vi", "PORT", "Done", ""),
    (r"dm/gfx/nvn/hle_nvn\.c$", "graphics::nvn::{entry,device,cmd}", "PORT", "Partial", "AUDIT: rows behind it still open (5A.4)"),
    (r"dm/gfx/nvn/(include/nvn\.h|names\.txt|nvn_protos\.h|nvn_count\.h)$", "graphics::nvn::{types,entry}", "PORT", "Done", ""),
    (r"dm/gfx/nvn/(nvn_thunks|nvn_entry)\.S$", "-", "DROP", "Dropped", "hostfn bridge (3.2)"),
    (r"dm/gfx/nvn/nvn_obj\.h$", "graphics::nvn::obj", "PORT", "Done", ""),
    (r"dm/gfx/nvn/nvn_mtl\.[hm]$", "graphics::vk::{device,mem,image,pipe,exec,queue,present}", "REWRITE", "Partial", "5A.4 rows open (lane G4)"),
    (r"dm/gfx/nvn/nvn_rec\.h$", "graphics::vk::exec", "REWRITE", "Partial", "recording on several threads open (5A.4)"),
    (r"dm/gfx/nvn/nvn_owner\.[hm]$", "graphics::vk::{queue,image}", "PORT", "Done", ""),
    (r"dm/gfx/nvn/nvn_queue\.(h|inc)$", "graphics::vk::queue, graphics::nvn::queue", "REWRITE", "Done", "timeline semaphores"),
    (r"dm/gfx/nvn/nvn_dirty\.[ch]$", "graphics::vk::lazy, graphics::nvn::vpool", "REWRITE", "Done", "write generations"),
    (r"dm/gfx/nvn/nvn_tex\.[ch]$", "graphics::tex", "PORT", "Done", ""),
    (r"dm/gfx/nvn/nvn_hmap\.h$", "-", "DROP", "Dropped", "HashMap"),
    (r"dm/gfx/nvn/nvn_(pace|link_drain|link_latch|fstep)\.h$", "graphics::vk::present", "PORT", "Partial", "frame step: 5A.6"),
    (r"dm/gfx/nvn/nvn_timestamp\.h$", "graphics::nvn::query", "PORT", "Done", ""),
    (r"dm/gfx/nvn/nvn_rt\.h$", "os::sched", "REWRITE", "Not started", "MMCSS / real-time thread class: only the priority_changed hook exists (AUDIT)"),
    (r"dm/gfx/nvn/nvn_pso_(ds|skey)\.h$", "graphics::vk::{pipe,pcache}", "PORT", "Done", ""),
    (r"dm/gfx/nvn/(nvn_refl\.[hm]|dmsh_abi\.h|dmsh_chain\.h)$", "shaders::reflect, graphics::vk::layout", "PORT", "Done", ""),
    (r"dm/gfx/nvn/(msl_image\.h|host_msl\.[hm])$", "-", "DROP", "Dropped", "Metal"),
    (r"dm/gfx/nvn/nvn_builtin\.h$", "graphics (shaders/)", "REWRITE", "Not started", "built-in counter kernels: none found (AUDIT)"),
    (r"dm/gfx/nvn/(nvn_border\.h|nvn_depth_clear\.h|nvn_wline\.inc)$", "-", "DROP", "Dropped", "native in Vulkan"),
    (r"dm/gfx/nvn/nvn_primitive\.h$", "graphics::vk::exec", "PORT", "Done", "quads"),
    (r"dm/gfx/nvn/(nvn_tess\.inc|nvn_geometry\.(h|inc)|nvn_feedback\.(h|inc))$", "graphics::vk::{exec,xfb}", "REWRITE", "Done", "native stages, transform feedback"),
    (r"dm/gfx/nvn/(nvn_accel\.(h|inc)|nvn_raytrace.*)$", "graphics::vk (rt)", "DEFER", "Deferred", "ray tracing: 5A.4 open"),
    (r"dm/gfx/nvn/nvn_ndve\.inc$", "graphics::nvn::cmd", "REWRITE", "Not started", "host-run movie / NDVE / NDSR network missing (AUDIT)"),
    (r"dm/gfx/nvn/nvn_capture\.(c|h|inc)$", "graphics::vk::shot, front::capture", "PORT", "Partial", "capture replay not ported"),
    (r"dm/gfx/msl/", "graphics (shaders/*.comp|frag)", "REWRITE", "Partial", "GLSL equivalents where needed"),
    (r"dm/gfx/metal/", "-", "DROP", "Dropped", "Metal"),
    (r"dm/gfx/(gl|vk)/", "-", "DEFER", "Deferred", "GL / Vulkan-API games"),
    (r"dm/gfx/tex/nvn_deswizzle\.[ch]$", "graphics::tex::deswizzle", "PORT", "Done", ""),
    (r"dm/gfx/tex/nvn_deswizzle_gpu\.[hm]$", "graphics::vk::image", "REWRITE", "Not started", "CPU deswizzle only (AUDIT)"),
    (r"dm/gfx/tex/nvn_fmt_mtl\.[ch]$", "graphics::tex::format", "REWRITE", "Done", "VkFormat table"),
    (r"dm/gfx/tex/tests/t_tex\.c$", "graphics::tex (tests)", "PORT", "Done", "t_tex_layout vectors"),
    (r"dm/gfx/tex/", "-", "DROP", "Dropped", "Metal format test / build files"),
    (r"dm/gfx/tests/", "graphics/tests", "PORT", "Partial", "selected; Metal probes dropped"),
    # dm/gfx/tools
    (r"dm/gfx/tools/shader_split\.py$", "shaders::{scan,formats}", "PORT", "Done", ""),
    (r"dm/gfx/tools/dmsh_(build|reflect|entry|side|vin|abi|facts|fetch|dryrun)\.py$", "shaders::{job,reflect,nvn}", "PORT", "Done", "MSL stages removed"),
    (r"dm/gfx/tools/dmsh_xlate/", "shaders::xlate (tools/build_dmsh_xlate.ps1)", "TOOL", "Done", "NativeAOT"),
    (r"dm/gfx/tools/dmsh_xlate_bin/", "-", "TOOL", "Done", "built locally, not vendored"),
    (r"dm/gfx/tools/dmsh_(rewrite|alpha|atomic|logic|wide|ampere)\.py$", "shaders::fixes", "PORT", "Partial", "Vulkan-relevant fixes only"),
    (r"dm/gfx/tools/dmsh_(geometry|mesh|meshstage|tess)\.py$", "shaders::mesh", "REWRITE", "Done", ""),
    (r"dm/gfx/tools/dmsh_rt.*\.py$", "shaders::rt", "DEFER", "Partial", "ray-query modules built; RT pipelines deferred"),
    (r"dm/gfx/tools/(dmsh_chain\.(py|lock)|tool_lock\.py|own_compilers\.py|kit\.py)$", "shaders::xlate", "PORT", "Done", ""),
    (r"dm/gfx/tools/(dmsh_pull\..*|dmsh_mtlcache\.py|dmsh_pch\.py|host_msl\.py|tbdr_.*\.py)$", "-", "DROP", "Dropped", "Metal"),
    (r"dm/gfx/tools/spirv_cross_bin/", "-", "DROP", "Dropped", "Metal (SPIR-V -> MSL)"),
    (r"dm/gfx/tools/pso_offline\.py$", "graphics::vk::pcache, profiles (pso tables)", "REWRITE", "Partial", "tables written into every build; prewarm_shaders has no caller (AUDIT)"),
    (r"dm/gfx/tools/gen_protos\.py$", "graphics::nvn::entry", "PORT", "Done", ""),
    (r"dm/gfx/tools/ampere_orin/", "shaders/tests", "PORT", "Not started", ""),
    (r"dm/gfx/tools/(vk_shaders|gl_shaders|gl_aliases|idtech4_programs|mvgl)\.py$", "-", "DEFER", "Deferred", "GL / Vulkan-API games"),
    (r"dm/gfx/tools/(c1_guard|cb_extent|shader_regressions|tex_census|passgpu_top)\.py$", "scripts (harness)", "PORT", "Not started", ""),
    (r"dm/gfx/tools/(casc|cpk|cra|d3cpk|frostbite|lspk|rpf6|smash_arc|uepak|unityfs|vbf|ttdat)\.py$", "shaders::formats", "PORT", "Partial", "containers the library's games use"),
    # ---- dm/tools
    (r"dm/tools/patch_nso\.py$", "translator::aot::patches, profiles::patches", "PORT", "Partial", "cuts / frame step as image patches; Apple rewrites dropped"),
    (r"dm/tools/(sdk_patches|engine_patches)\.py$", "profiles (shared/*.toml)", "DATA", "Done", "tools/export.py"),
    (r"dm/tools/nso_match\.py$", "profiles::patches", "PORT", "Not started", "fingerprint search for match rules"),
    (r"dm/tools/jump_solver\.py$", "translator::xlate::jumptables (tests)", "REF", "Reference", "mkw-recomp jumptables is the implementation"),
    (r"dm/tools/nso_asm\.py$", "profiles tools/export.py", "PORT", "Done", "encoders"),
    (r"dm/tools/(nso|nso_code|nso_fnmap|nso_syms|nso2elf|sdis)\.py$", "translator::loader", "PORT", "Done", ""),
    (r"dm/tools/(insn_census|loader_census|coverage_report)\.py$", "translator::aot (coverage)", "PORT", "Partial", ""),
    (r"dm/tools/nro_collect\.py$", "translator::package::nro", "PORT", "Done", ""),
    (r"dm/tools/jit_capture\.py$", "-", "DROP", "Dropped", "iOS"),
    (r"dm/tools/(romfs|nacp)\.py$", "translator::package::{romfs,cnmt}", "PORT", "Done", ""),
    (r"dm/tools/(romfs_image|tree_tar|vol_space)\.py$", "-", "DROP", "Dropped", "iOS packaging"),
    (r"dm/tools/nx_extract(_bin)?/", "translator::package", "REWRITE", "Done", "Turbo's in-place reading"),
    (r"dm/tools/(shader_cache|shader_lib)\.py$", "shaders::prebuild", "PORT", "Done", ""),
    (r"dm/tools/make_(shared|ext)_fonts?\.py$", "os::services::misc (shared font)", "PORT", "Partial", ""),
    (r"dm/tools/(ue_.*|unity_.*|il2cpp_sym|snowdrop_pools)\.py$", "profiles (engines)", "DEFER", "Deferred", ""),
    (r"dm/tools/(fps_target|monolith_fps)\.py$", "profiles, os::gamefix", "PORT", "Partial", "5A.6"),
    (r"dm/tools/(check_thread_cuts|audio_channel_check|gpuprof|triage_report|usage|proc_group|run_saves)\.py$", "scripts (harness)", "PORT", "Not started", "as needed"),
    (r"dm/tools/(docs_patches|docs_symptoms|env_switches|issue_areas|pr_areas)\.py$", "scripts (docs)", "PORT", "Not started", ""),
    (r"dm/tools/impact_map\.tsv$", "scripts (docs)", "DATA", "Not started", ""),
    (r"dm/tools/lint_.*$", "-", "DROP", "Dropped", "clippy"),
    (r"dm/tools/(nvn_native_sizes|nvn_slots_got)\.py$", "graphics::nvn (tests)", "PORT", "Partial", ""),
    (r"dm/tools/(ghidra/|native_tools\.sh)", "-", "DROP", "Dropped", ""),
    (r"dm/tools/fuzz/", "cargo-fuzz corpus", "PORT", "Not started", ""),
    (r"dm/tools/tests/", "crate tests", "PORT", "Partial", "re-expressed per module"),
    # ---- dm/games
    (r"dm/games/[0-9A-F]{16}/game\.tsv$", "profiles (games/<id>/game.tsv)", "DATA", "Done", "byte for byte"),
    (r"dm/games/[0-9A-F]{16}/patches\.py$", "profiles (games/<id>/patches.toml)", "DATA", "Done", "exported"),
    (r"dm/games/[0-9A-F]{16}/(pso_manifest|pso_pairs|shader_pairs|shader_facts)\.tsv$", "profiles (games/<id>/shaders.tsv.zst)", "DATA", "Done", "export_shaders.py"),
    (r"dm/games/[0-9A-F]{16}/README\.md$", "-", "DATA", "Reference", "game notes, read in place"),
    (r"dm/games/[0-9A-F]{16}/", "-", "DATA", "Reference", "evidence / maps not imported (shader_map.tsv, captures)"),
    (r"dm/games/game\.py$", "profiles", "PORT", "Partial", "profiles parse; several keys (fps_target) unread at run time"),
    (r"dm/games/settings\.py$", "profiles", "PORT", "Partial", "player_settings() has no consumer (AUDIT)"),
    (r"dm/games/settings\.tsv$", "profiles (shared/settings.tsv)", "DATA", "Done", ""),
    (r"dm/games/profile_guess\.py$", "profiles", "PORT", "Not started", ""),
    (r"dm/games/README\.md$", "src/profiles/README.md", "REWRITE", "Done", ""),
    # ---- the rest
    (r"scripts/build_ipa_from_nsp\.py$", "cli::pipeline", "PORT", "Done", "IPA dropped"),
    (r"scripts/(pack_nup|pack_dump)\.py$", "translator::package", "PORT", "Partial", "reading only"),
    (r"scripts/(regress|bench)\.py$", "scripts (harness)", "PORT", "Partial", "run_game.py / play_scenario.py"),
    (r"scripts/bug_report\.py$", "scripts (harness)", "PORT", "Not started", ""),
    (r"scripts/cheat_anchor\.py$", "-", "DEFER", "Deferred", ""),
    (r"scripts/(clean_metal_cache\.py|push_romfs\.sh)$", "-", "DROP", "Dropped", ""),
    (r"dm/(mac|ios|sanitizers)/", "-", "DROP", "Dropped", "Apple / sanitizer builds"),
    (r"docs/reference/", "docs (reference)", "DATA", "Not started", "to be carried over in the same format"),
    (r"docs/images/", "-", "DROP", "Dropped", ""),
    (r"docs/", "docs", "REWRITE", "Partial", "PORT_PLAN / INTERFACES / READMEs"),
    (r"dm/third_party/ryujinx/", "shaders::xlate (dmsh_xlate)", "TOOL", "Done", ""),
    (r"dm/third_party/dav1d/", "os::media::av1", "TOOL", "Done", "built library"),
    (r"dm/third_party/libvpx/", "os::media::vpx", "TOOL", "Done", "built library"),
    (r"dm/third_party/stb_vorbis/", "os::media", "TOOL", "Partial", ""),
    (r"dm/third_party/flip/", "scripts (harness)", "TOOL", "Not started", ""),
    (r"dm/third_party/vulkan-headers/", "graphics (ash)", "DROP", "Dropped", "ash"),
    (r"dm/third_party/", "-", "REF", "Reference", "notices kept in LICENSES/ where used"),
    (r"\.github/", "-", "DROP", "Dropped", "own CI: .github/workflows/ci.yml"),
    (r"(\.gitignore|dm/\.gitignore|\.clang-tidy|pyproject\.toml)$", "-", "DROP", "Dropped", ""),
    (r"(README\.md|AGENTS\.md|CONTRIBUTING\.md|LICENSE)$", "README.md, LICENSES/", "REF", "Reference", "credits"),
]

TB_OWNER = "the owner's fork code (MIT)"
TB = [
    (r"(nca|nsp|nup|dlc|zbic|nxloader)\.py$", "translator::package, translator::loader", "PORT", "Done", ""),
    (r"switchrecomp\.py$", "cli", "PORT", "Partial", "options mapped (src/cli/README.md); SWR_CFLAGS, hints / interpreter fallback, --target linux missing (AUDIT)"),
    (r"(a64|codegen|brt|tools_sym)\.py$", "translator::xlate (mkw-recomp lifter instead)", "DROP", "Dropped", "Turbo's own translator; the lifter is mkw-recomp's"),
    (r"runtime/(nsp|nup)\.c$", "translator::package", "PORT", "Done", "in-place reading"),
    (r"runtime/dlc\.c$", "translator::package::dlc, os::services::fs", "PORT", "Partial", "DLC whose data is missing is still offered (AUDIT)"),
    (r"runtime/(kernel|svc_core|mem|ipc|loader|entry|cpu_helpers|plat|glibc_compat)\.c$", "runtime, os", "REF", "Reference", "MZ Labs; nxnative's host is the base"),
    (r"runtime/svc_(fs|hid|ns2|vi)\.c$", "os::services", "REF", "Reference", "MZ Labs; nxnative's services are the base"),
    (r"runtime/(audio\.[ch]|audio_limit\.h|audio_queue\.h)$", "os::services::audren, os::media::sink", "REF", "Reference", ""),
    (r"runtime/main\.c$", "config::options (option table)", "PORT", "Partial", "every option in nxconfig; renderer=opengl/eden silently becomes vulkan (owner decision)"),
    (r"runtime/backend_win32\.c$", "front::{window,keys}", "PORT", "Partial", "hotkeys, window; F5 dump is a stub (AUDIT)"),
    (r"runtime/backend_(x11|headless)\.c$", "-", "DEFER", "Deferred", "Linux (M10); headless dropped (real I/O)"),
    (r"runtime/(controller_sony(_win32)?\.h|pad_layout\.h|hid_layout\.h)$", "os::input::{sony,layout}", "PORT", "Done", ""),
    (r"runtime/(rec_win32\.c|rec_flow\.h|rec_audio_time\.h)$", "front::rec", "PORT", "Partial", "F7 recording; stall watchdog and original-timeline audio missing"),
    (r"runtime/lsfg.*$", "front::lsfg, graphics::vk::lsfg", "PORT", "Done", ""),
    (r"runtime/(render_scale|render_condition)\.h$", "graphics::vk::scale", "PORT", "Done", ""),
    (r"runtime/(gpu.*|host1x|nvdrv|gl_load|glq.*|glfn|nvn_glue|nvn_hook|shader|shader_proc|sm86|astc)\.(c|h)$", "graphics", "REF", "Reference", "Turbo's GPU path; nxnative's NVN layer is the base"),
    (r"runtime/edenvk.*", "-", "DROP", "Dropped", "Eden Vulkan renderer (renderer=vulkan); nxrs replays NVN on Vulkan itself"),
    (r"runtime/(av1|vp9|opus|oh264)/", "os::media", "TOOL", "Done", "codec libraries built for os (build.rs)"),
    (r"runtime/(av1video|vp9video)\.inc$", "os::media::{av1,vpx}", "PORT", "Done", ""),
    (r"runtime/bcn/", "graphics::tex", "REF", "Reference", ""),
    (r"runtime/spvc/", "-", "DROP", "Dropped", "SPIRV-Cross for the GL path"),
    (r"runtime/(swkbd_font|splash_img)\.h$", "os::services::swkbd, front", "REF", "Reference", ""),
    (r"runtime/(cpu|rt)\.h$", "runtime::cpu", "REF", "Reference", "mkw-recomp's cpu.h is the ABI"),
    (r"runtime/nvn/vk/vk_ctl\.c$", "front::ctl, config::ctl", "PORT", "Partial", "graphics' pause/step/passdump commands (lane G4)"),
    (r"runtime/nvn/vk/vk_(mem|sparse)\.(c|h|inc)$", "graphics::vk::{mem,sparse,gmem}", "PORT", "Done", ""),
    (r"runtime/nvn/vk/vk_shot\.[ch]$", "graphics::vk::shot", "PORT", "Done", ""),
    (r"runtime/nvn/vk/vk_(pcache|pipe)\.[ch]$", "graphics::vk::{pipe,pcache}", "PORT", "Done", ""),
    (r"runtime/nvn/vk/vk_(perf|flight|aftermath)\.[ch]$", "graphics::vk::{perf,counters}", "PORT", "Partial", ""),
    (r"runtime/nvn/vk/vk_present\.[ch]$", "graphics::vk::present", "PORT", "Done", ""),
    (r"runtime/nvn/vk/vk_(tex|fmt|astc)\.(c|h|cpp)$", "graphics::{tex,vk::image}", "PORT", "Done", ""),
    (r"runtime/nvn/vk/vk_(shader)\.[ch]$", "graphics::vk::{shader,xfb}", "PORT", "Done", ""),
    (r"runtime/nvn/vk/vk_(submit|replay|defer|dev|window|fn|json|hmap|internal)\.[ch]$", "graphics::vk", "PORT", "Partial", "5A.4 (lane G4)"),
    (r"runtime/nvn/vk/gfx_vk\.c$", "graphics::vk", "PORT", "Partial", "5A.4 (lane G4)"),
    (r"runtime/nvn/vk/shaders/test_", "graphics/tests", "PORT", "Partial", ""),
    (r"runtime/nvn/vk/shaders/", "graphics (shaders/)", "PORT", "Done", ""),
    (r"runtime/nvn/vk/third_party/", "graphics (astc_decode / bc7_encode compute)", "DROP", "Dropped", "GPU transcode instead"),
    (r"runtime/nvn/vk/CMakeLists\.txt$", "-", "DROP", "Dropped", "Cargo"),
    (r"runtime/nvn/(gfx|gen|host|s2tex)/", "graphics::nvn, os", "REF", "Reference", "nxnative's NVN layer is the base"),
    (r"runtime/nvn/hle_nvn\.c$", "graphics::nvn", "REF", "Reference", ""),
    (r"runtime/nvn/third_party/", "LICENSES/", "REF", "Reference", ""),
    (r"runtime/edenvk_stub\.c$", "-", "DROP", "Dropped", ""),
    (r"tools/play_scenario\.py$", "scripts/play_scenario.py", "PORT", "Done", ""),
    (r"tools/plza_inputs/", "scripts/inputs", "DATA", "Done", ""),
    (r"tools/nxshader/pipelines/", "profiles (shaders.tsv.zst)", "DATA", "Done", "nxnative tables are the source"),
    (r"tools/nxshader/xlate_bin/", "shaders::xlate (dmsh_xlate)", "TOOL", "Done", ""),
    (r"tools/nxshader/", "shaders", "PORT", "Done", "prebuild"),
    (r"tools/(mkw_.*|plza_probe|host_prof|build_nvn|s2r_paths)\.py$", "scripts", "REF", "Reference", "game probes"),
    (r"tools/hactool/", "-", "DROP", "Dropped", ""),
    (r"tool/", "-", "DROP", "Dropped", "Turbo's own launcher tool (the Qt launcher is separate)"),
    (r"scripts/", "-", "DROP", "Dropped", "Turbo build scripts; nxrs-build replaces them"),
    (r"(build_runtime\.bat|Install\.(bat|sh)|SwitchRecomp)$", "-", "DROP", "Dropped", ""),
    (r"tests/", "crate tests", "REF", "Reference", "Turbo's test vectors"),
    (r"validation/", "-", "REF", "Reference", ""),
    (r"docs/images/", "-", "DROP", "Dropped", ""),
    (r"docs/", "docs", "REF", "Reference", "investigations read by the lanes"),
    (r"\.github/", "-", "DROP", "Dropped", "own CI"),
    (r"(\.gitattributes|\.gitignore|CHANGELOG\.txt|DEVELOPER_GUIDE\.md|LICENSE\.txt|NOTICE\.txt|README\.(md|txt)|REGRESSION\.md)$", "README.md, LICENSES/", "REF", "Reference", ""),
]

# mkw-recomp donor files (section 4.7): which paths are part of the map, then the rules.
MR_FILES = [
    "recomp/lifter.py", "recomp/simd_c.py", "recomp/flags.py", "recomp/regs.py", "recomp/jumptables.py",
    "recomp/gen.py", "recomp/nso.py", "recomp/module.py", "recomp/layout.py", "recomp/patches.py",
    "recomp/versions.py", "recomp/patches-mkw.txt", "recomp/patches-mkw-180.txt",
    "tools/prepare/gamecode.py", "tools/prepare/fastcc.py", "tools/cpu/spin_waits.py", "tools/cpu/lift_coverage.py",
    "runtime/cpu/", "runtime/core/kernel/kernel_fiber.c", "runtime/core/kernel/kernel_preempt.c",
    "runtime/platform/win32_sync.c", "runtime/platform/win32_sys.c", "runtime/core/memory/memory.c",
    "runtime/core/memory/memory_early.c", "runtime/platform/win32_mem.c", "runtime/gfx/vk/", "runtime/gfx/cap/",
    "runtime/shaders/texture/", "runtime/shaders/fsr/", "runtime/shaders/ffx/", "runtime/input/switch2_win.c", "sm86/",
]
MR = [
    (r"recomp/(lifter|simd_c|flags|regs|jumptables)\.py$", "translator::xlate", "DONOR", "Done", "byte-identical C (M1)"),
    (r"recomp/(gen|nso|module|layout|patches)\.py$", "translator::{aot,loader}", "DONOR", "Done", "layout base per profile"),
    (r"recomp/(versions\.py|patches-mkw.*\.txt)$", "-", "REF", "Reference", "profiles come from nxnative"),
    (r"tools/prepare/(gamecode|fastcc)\.py$", "cli::cc", "DONOR", "Done", ""),
    (r"tools/cpu/(spin_waits|lift_coverage)\.py$", "translator::aot", "DONOR", "Done", ""),
    (r"runtime/cpu/(cpu\.h|simd\.h|simd_fp\.h|fma\.c)$", "runtime (include/), translator::aot::runtime_headers", "DONOR", "Done", "used as C"),
    (r"runtime/cpu/(dispatch\.[ch]|hostfn\.[ch]|sjlj\.[ch])$", "runtime::{dispatch,hostfn,sjlj}", "DONOR", "Done", ""),
    (r"runtime/cpu/", "runtime", "REF", "Reference", ""),
    (r"runtime/core/kernel/kernel_fiber\.c$", "runtime::fiber", "DONOR", "Done", ""),
    (r"runtime/(core/kernel/kernel_preempt|platform/win32_sync|platform/win32_sys)\.c$", "runtime::kernel, os::sched", "REF", "Reference", ""),
    (r"runtime/(core/memory/memory|core/memory/memory_early|platform/win32_mem)\.c$", "runtime::mem", "DONOR", "Done", ""),
    (r"runtime/gfx/vk/mem/", "graphics::vk::{mem,gmem,lazy}", "DONOR", "Done", ""),
    (r"runtime/gfx/vk/pipe/", "graphics::vk::{pipe,pcache,spvfix}", "DONOR", "Done", ""),
    (r"runtime/gfx/vk/(exec|tex|device)/", "graphics::vk::{exec,image,device,queue,present}", "REF", "Reference", "selected code"),
    (r"runtime/gfx/vk/", "graphics::vk", "REF", "Reference", ""),
    (r"runtime/gfx/cap/", "graphics::vk::shot", "REF", "Reference", "nxnative's capture format stays"),
    (r"runtime/shaders/(texture|fsr|ffx)/", "graphics (shaders/)", "DONOR", "Partial", "ASTC/BC7 compute in use; FSR not yet"),
    (r"runtime/input/switch2_win\.c$", "os::input::switch2_ble", "DONOR", "Done", ""),
    (r"sm86/", "-", "REF", "Reference", "cross-check only (3.4)"),
]


def git_files(root):
    out = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True, check=True).stdout
    return [p for p in out.decode("utf-8", "replace").split("\0") if p]


def git_head(root):
    return subprocess.run(["git", "-C", str(root), "rev-parse", "--short", "HEAD"], capture_output=True, text=True).stdout.strip()


def mr_files(root):
    files = []
    for f in MR_FILES:
        p = root / f
        if f.endswith("/"):
            if p.is_dir():
                files += sorted(str(q.relative_to(root)).replace("\\", "/") for q in p.rglob("*")
                                if q.is_file() and "__pycache__" not in q.parts)
        elif p.is_file():
            files.append(f)
    return files


def lines(path):
    try:
        with open(path, "rb") as fh:
            return sum(chunk.count(b"\n") for chunk in iter(lambda: fh.read(1 << 20), b""))
    except OSError:
        return 0


def rows_for(tree, root, files, rules):
    compiled = [(re.compile(r), d, a, s, n) for r, d, a, s, n in rules]
    out = []
    for f in sorted(files):
        for rx, d, a, s, n in compiled:
            if rx.match(f):
                break
        else:
            d, a, s, n = "UNMAPPED", "UNMAPPED", "UNMAPPED", "no rule"
        out.append((tree, f, str(lines(root / f)), d, a, s, n))
    return out


def find(arg, *defaults):
    if "--format-only" in sys.argv:
        return None
    if arg:
        return Path(arg) if Path(arg).is_dir() else None
    for d in defaults:
        if Path(d).is_dir():
            return Path(d)
    return None


def main():
    argv = sys.argv[1:]
    opt = lambda k: argv[argv.index(k) + 1] if k in argv else None
    check = "--check" in argv
    trees = {
        "NX": find(opt("--nx"), ROOT / "build/Q/nxnative-up", "D:/Projects/nxnative"),
        "TB": find(opt("--turbo"), "D:/Projects/Switch2RECOMP-main"),
        "MR": find(opt("--mr"), "D:/Projects/mkw-recomp-main"),
    }
    gen = {}
    heads = {}
    for t, root in trees.items():
        if root is None:
            continue
        if t == "MR":
            files = mr_files(root)
            heads[t] = "files of section 4.7"
        else:
            files = git_files(root)
            heads[t] = git_head(root)
        gen[t] = rows_for(t, root, files, {"NX": NX, "TB": TB, "MR": MR}[t])

    if not check:
        missing = [t for t in trees if t not in gen]
        if missing:
            sys.exit(f"tree(s) not found: {missing}; pass --nx/--turbo/--mr")
        with open(OUT, "w", encoding="utf-8", newline="\n") as fh:
            fh.write("# PORT_MAP.tsv: every file of the trees nxrs ports from -> its place in nxrs (docs/PORT_PLAN.md section 4).\n")
            fh.write(f"# NX = nxnative {heads['NX']}, TB = Turbo (Switch2RECOMP-main, branch turbo) {heads['TB']}, MR = mkw-recomp-main ({heads['MR']}).\n")
            fh.write("# Regenerate / verify: py -I scripts/port_map.py [--check]\n")
            fh.write("tree\tpath\tlines\tdest\taction\tstatus\tnote\n")
            for t in ("NX", "TB", "MR"):
                for r in gen[t]:
                    fh.write("\t".join(r) + "\n")
        n = sum(len(v) for v in gen.values())
        un = sum(1 for v in gen.values() for r in v if r[4] == "UNMAPPED")
        print(f"PORT_MAP.tsv: {n} rows ({', '.join(f'{t} {len(v)}' for t, v in gen.items())}), {un} unmapped")
        return 1 if un else 0

    # --check
    errors = []
    have = {}
    with open(OUT, encoding="utf-8") as fh:
        body = [l.rstrip("\n") for l in fh if not l.startswith("#")]
    if not body or body[0] != "tree\tpath\tlines\tdest\taction\tstatus\tnote":
        errors.append("bad header")
    for i, l in enumerate(body[1:], 2):
        c = l.split("\t")
        if len(c) != 7:
            errors.append(f"row {i}: {len(c)} columns")
            continue
        t, p, n, d, a, s, _ = c
        if t not in ("NX", "TB", "MR") or not n.isdigit() or not d or a not in ACTIONS or s not in STATUSES:
            errors.append(f"row {i}: {l[:120]}")
        if (t, p) in have:
            errors.append(f"duplicate {t} {p}")
        have[(t, p)] = c
    for t, rows in gen.items():
        want = {(r[0], r[1]): list(r) for r in rows}
        mine = {k: v for k, v in have.items() if k[0] == t}
        for k in want.keys() - mine.keys():
            errors.append(f"no row: {k[0]} {k[1]}")
        for k in mine.keys() - want.keys():
            errors.append(f"row for a file not in the tree: {k[0]} {k[1]}")
        for k in want.keys() & mine.keys():
            if want[k] != mine[k]:
                errors.append(f"differs: {k[0]} {k[1]}: {mine[k][2:6]} vs {want[k][2:6]}")
    for e in errors[:40]:
        print(e)
    print(f"PORT_MAP.tsv: {len(have)} rows; trees checked against: {sorted(gen) or 'none (format only)'}; {len(errors)} problems")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
