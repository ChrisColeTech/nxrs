import json
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent))
import observability  # noqa: E402


def write_png(path, width, height, pixels):
    raw = b"".join(b"\0" + b"".join(bytes(pixel) for pixel in row) for row in pixels)

    def chunk(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xffffffff)

    path.write_bytes(b"\x89PNG\r\n\x1a\n" +
                     chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)) +
                     chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


class ObservabilitySummary(unittest.TestCase):
    def test_partial_run_summary_and_manifest(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "run.log").write_text(
                "[vk] shader abc is not in the shader library\n"
                "[vk] live shader abc translated in 0.20 s (0.3 s after it was missed)\n"
                "[os] UNIMPL Service foo:u: answered 0\n"
                "[os] gap summary (exit): 1 distinct\n[os]   gap Service foo:u x3\n"
                "[os] audio: game: 3.0 s played, 2 underruns, slowed down 0.0 s\n"
                "[vk] slow frame 7: 75 ms: replay 2 ms\n"
                "[RED] the GPU stopped responding (device lost, first seen at wait); what it was doing:\n"
                "[RED]   the GPU started #9 a dispatch (stage 0x1)\n"
                "[RED]   fault address: invalid read at 0x1234 (within 0x10)\n"
            )
            (run / "frames-1.csv").write_text(
                "#present,guest_s,frame_ms,submits,nvn_calls,threads,draws,draws_skipped\n"
                "1,0,10,1,2,3,4,0\n2,.02,20,1,2,3,4,2\n3,.08,70,1,2,3,4,1\n"
            )
            (run / "metrics-1.csv").write_text(
                "#seconds,frames,fps,cpu_percent,private_mb,xmon,pipelines_gfx,pipeline_ns\n"
                "0,0,0,0,100,0,2,1000000\n60,60,1,0,250,0,3,2000000\n"
            )
            (run / "0001-boot.png").write_bytes(b"png")
            summary = observability.generate(run, {"why": "timeout", "exit": 1}, (0, 60))
            self.assertEqual(summary["schema"], "nxrs.run-summary.v1")
            self.assertEqual(summary["performance"]["frame_time_ms"]["p95"], 65.0)
            self.assertEqual(summary["performance"]["late_frames"], 2)
            self.assertEqual(summary["performance"]["stutters"], 1)
            self.assertEqual(summary["performance"]["dropped_work"], 3)
            self.assertTrue(summary["memory"]["leak_warning"])
            self.assertEqual(summary["shaders"]["live_translated_unique"], 1)
            self.assertEqual(summary["gaps"]["hits"], 3)
            self.assertEqual(summary["audio"]["underruns"], 2)
            self.assertTrue(summary["gpu"]["device_lost"])
            self.assertEqual(summary["red_diagnostics"]["unique"], 3)
            self.assertEqual(summary["red_diagnostics"]["cross_referenced_unique"], 3)
            self.assertEqual(summary["red_diagnostics"]["categories"]["device_gpu"]["count"], 3)
            manifest = json.loads((run / "artifact-manifest.json").read_text())
            self.assertIn("0001-boot.png", [a["path"] for a in manifest["artifacts"]])
            self.assertTrue((run / "summary.json").is_file())

    def test_harness_memory_fallback(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "harness-memory.csv").write_text("seconds,private_mb,free_commit_mb\n0,90,50000\n120,110,49000\n")
            summary = observability.generate(run)
            self.assertEqual(summary["memory"]["slope_mb_per_min"], 10.0)
            self.assertFalse(summary["memory"]["leak_warning"], "small growth alone is not a leak warning")

    def test_legacy_metrics_empty_frames_pacing_and_dead_memory_sample(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "metrics-123.csv").write_text(
                "0.0,0,0.0,0,83\n19.0,989,60.0,20,1792\n20.0,1047,58.0,27,1792\n"
                "30.0,1645,60.0,16,1802\n50.0,2845,60.0,16,1900\n")
            (run / "frames-123.csv").write_text("")
            (run / "harness-memory.csv").write_text(
                "seconds,private_mb,free_commit_mb\n2.5,1349,50000\n20,1792,49000\n30,1802,49000\n"
                "40,1812,49000\n48.2,0,51000\n")
            (run / "run.log").write_text(
                "[os 1.0] gap summary (first submit): 0 distinct\n"
                "[os 5.0] audio: output on Speakers (Wasapi, 48000 Hz, 2 channels, F32); smooth slowdown on\n"
                "[os 31.0] audio: renderer 0: 29.0 s played (25.0 s not silent), 0 underruns, slowed down 0.0 s\n"
                "[ 21.28] [vk] pacing: 59.24 FPS | p95 17.05 p99 18.08 max 66.15 ms | "
                ">16.67 147 >33/50/100 2/2/0 (last 297 intervals)\n"
                "[ 26.30] [vk] pacing: 59.62 FPS | p95 16.97 p99 17.11 max 64.85 ms | "
                ">16.67 152 >33/50/100 1/1/0 (last 299 intervals)\n")

            summary = observability.generate(run, window=(20, 40))

            self.assertEqual(summary["performance"]["metrics_fps"]["samples"], 2)
            self.assertEqual(summary["performance"]["source"], "pacing_log_estimate")
            self.assertEqual(summary["performance"]["frames"], 596)
            self.assertEqual(summary["performance"]["stutters"], 3)
            self.assertIsNone(summary["performance"]["dropped_frames"])
            self.assertIsNone(summary["performance"]["late_frames"])
            self.assertEqual(summary["performance"]["pacing"]["over_16_67_ms"], 299)
            self.assertIn("estimated", summary["performance"]["stutters_method"])
            self.assertEqual(summary["memory"]["samples"], 3)
            self.assertEqual(summary["memory"]["start_mb"], 1792.0)
            self.assertEqual(summary["memory"]["end_mb"], 1812.0)
            self.assertEqual(summary["memory"]["slope_mb_per_min"], 60.0)
            self.assertTrue(summary["gaps"]["observed"])
            self.assertEqual(summary["gaps"]["distinct"], 0)
            self.assertTrue(summary["audio"]["output_active"])
            self.assertTrue(summary["audio"]["non_silent"])
            self.assertEqual(summary["audio"]["non_silent_seconds"], 25.0)

    def test_scenario_acceptance_and_png_diagnostics_pass(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            acceptance = {
                "target_fps": 60, "gameplay_window": [10, 20], "min_fps": 58,
                "max_p95_ms": 17, "max_p99_ms": 18, "max_stutters": 0, "allowed_gaps": 5.5,
                "memory_slope_threshold": 10, "max_memory_peak_mb": 150,
                "required_screenshot_names": ["start", "0002-end.png"],
                "image_thresholds": {
                    "defaults": {"max_black_fraction": 0.1, "min_coarse_unique_colours": 1},
                    "screenshots": {"0002-end.png": {"min_frame_difference": 10, "max_dominant_purple_fraction": 0.1}}
                }
            }
            (run / "scenario.json").write_text(json.dumps({"schema": "nxrs.scenario.v1", "scenario": "synthetic",
                                                             "acceptance": acceptance}))
            (run / "run.log").write_text(
                "[ 10.0] [vk] pacing: 62 FPS | p95 16 p99 17 max 18 ms\n"
                "[ 15.0] [vk] pacing: 61 FPS | p95 16 p99 17 max 18 ms\n")
            (run / "frames-1.csv").write_text(
                "#present,guest_s,frame_ms,draws_skipped\n1,10,16,0\n2,15,16,0\n3,20,16,0\n")
            (run / "metrics-1.csv").write_text(
                "#seconds,fps,private_mb\n10,60,100\n15,59,100.5\n20,61,101\n")
            red = [[(200, 20, 20)] * 2 for _ in range(2)]
            green = [[(20, 200, 20)] * 2 for _ in range(2)]
            write_png(run / "0001-start.png", 2, 2, red)
            write_png(run / "0002-end.png", 2, 2, green)
            (run / "actions.jsonl").write_text(
                json.dumps({"screenshot": "0001-start.png", "name": "start"}) + "\n" +
                json.dumps({"screenshot": "0002-end.png", "name": "end"}) + "\n")

            summary = observability.generate(run, {"exit": 0, "why": "exited", "seconds": 20})

            self.assertEqual(summary["run"]["window_seconds"], [10, 20])
            self.assertEqual(summary["verdict"]["status"], "pass")
            self.assertTrue(summary["verdict"]["pass"])
            self.assertTrue(all(check["status"] == "pass" for check in summary["verdict"]["checks"]))
            first, second = summary["screenshots"]
            self.assertEqual(first["mean_rgb"], [200.0, 20.0, 20.0])
            self.assertEqual(first["coarse_unique_colours"], 1)
            self.assertEqual(second["previous_frame"], "0001-start.png")
            self.assertEqual(second["changed_pixel_fraction"], 1.0)
            self.assertEqual(second["mean_abs_rgb_difference"], 120.0)
            self.assertEqual(len(second["pixel_sha256"]), 64)

    def test_fail_and_unknown_do_not_pass(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {"target_fps": 60,
                                                                            "required_screenshot_names": []}}))
            summary = observability.generate(run)
            self.assertEqual(summary["verdict"]["status"], "unknown")
            self.assertFalse(summary["verdict"]["pass"])
            self.assertTrue(any(check["status"] == "unknown" for check in summary["verdict"]["checks"]))

            (run / "run.log").write_text("[vk] live shader dead: translation failed\n")
            summary = observability.generate(run)
            self.assertEqual(summary["verdict"]["status"], "fail")
            failed = {check["name"] for check in summary["verdict"]["checks"] if check["status"] == "fail"}
            self.assertIn("shaders.max_failures", failed)
            self.assertFalse(summary["verdict"]["pass"])

    def test_red_diagnostics_are_normalized_categorized_and_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {
                "required_screenshot_names": []}}))
            (run / "run.log").write_text(
                "[ 1.25] [RED] GPU address 0x1234 (+0x40) is in no memory pool\n"
                "[ 2.50] [RED] GPU address 0x5678 (+0x40) is in no memory pool\n"
                "[ 2.75] [RED] shader 1bd2849fe5e9f00c (stage 1): uniform bank c3 at 0x1234 "
                "(+0xc0) is in no memory pool: zeros bound\n"
                "[ 2.80] [RED] shader 1bd2849fe5e9f00c (stage 1): uniform bank c3 at 0x5678 "
                "(+0xc0) is in no memory pool: zeros bound\n"
                "[ 3.00] [RED] texture ID 6666: target 4 bound where the shader samples target 8\n"
                "[ 4.00] [RED] 2 pipeline(s) were compiling when the last session ended (a compiler crash): "
                "not compiled again (pipeline-crashed.bin)\n"
                "[ 5.00] [RED] Validation error VUID-test\n"
                "[ 6.00] [RED] unexpected engine failure\n")

            summary = observability.generate(run)

            red = summary["red_diagnostics"]
            self.assertEqual(red["total_lines"], 8)
            self.assertEqual(red["unique"], 6)
            self.assertEqual(red["unallowed_unique"], 6)
            binding = red["categories"]["shader_resource_binding"]
            self.assertEqual((binding["count"], binding["occurrences"]), (3, 5))
            self.assertEqual(binding["items"][0]["first_timestamp"], 1.25)
            self.assertIn("0x1234", binding["items"][0]["representative"])
            self.assertIn("<hex>", binding["items"][0]["normalized"])
            self.assertEqual(red["categories"]["pipeline_crash_blacklist_compiler"]["count"], 1)
            self.assertEqual(red["categories"]["validation"]["count"], 1)
            self.assertEqual(red["categories"]["other"]["count"], 1)
            check = next(c for c in summary["verdict"]["checks"]
                         if c["name"] == "diagnostics.max_red_errors")
            self.assertEqual((check["actual"], check["status"]), (6, "fail"))

    def test_red_allowances_and_cross_references_do_not_double_count(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {
                "max_red_errors": 0,
                "allowed_red_categories": ["validation"],
                "allowed_red_patterns": ["expected stand-in"],
                "required_screenshot_names": []}}))
            (run / "run.log").write_text(
                "[RED] the GPU stopped responding (device lost, first seen at wait)\n"
                "[RED] shader dead: translation failed\n"
                "[RED] UNIMPL Service foo:u\n"
                "[RED] Validation error VUID-allowed\n"
                "[RED] resource expected stand-in installed\n")

            summary = observability.generate(run)

            red = summary["red_diagnostics"]
            self.assertEqual(red["unique"], 5)
            self.assertEqual(red["cross_referenced_unique"], 3)
            self.assertEqual(red["allowed_unique"], 2)
            self.assertEqual(red["unallowed_unique"], 0)
            check = next(c for c in summary["verdict"]["checks"]
                         if c["name"] == "diagnostics.max_red_errors")
            self.assertEqual((check["actual"], check["status"]), (0, "pass"))

    def test_target_fps_uses_unchanged_default_tolerance(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {
                "target_fps": 60, "gameplay_window": [10, 20], "required_screenshot_names": []}}))
            (run / "run.log").write_text("[ 15.0] [vk] pacing: 59.6 FPS | p95 17 p99 18 max 19 ms\n")
            summary = observability.generate(run)
            check = next(c for c in summary["verdict"]["checks"]
                         if c["name"] == "performance.expected_fps")
            self.assertEqual(check["expected"], {"min": 58.8})
            self.assertEqual(check["status"], "pass")

    def test_audio_underruns_are_delta_within_gameplay_window(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {
                "gameplay_window": [60, 720], "max_audio_underruns": 0,
                "required_screenshot_names": []}}))
            (run / "run.log").write_text(
                "[os   50.0] audio: AudioOut #1: 45 s played, 69 underruns, slowed down 4 s\n"
                "[os  121.9] audio: AudioOut #1: 116 s played, 69 underruns, slowed down 4 s\n"
                "[os  721.9] audio: AudioOut #1: 716 s played, 70 underruns, slowed down 4 s\n")
            summary = observability.generate(run)
            self.assertEqual(summary["audio"]["underruns"], 0)
            self.assertEqual(summary["audio"]["underruns_gameplay_window"], 0)
            self.assertEqual(summary["audio"]["underruns_lifetime"], 70)
            check = next(c for c in summary["verdict"]["checks"] if c["name"] == "audio.max_underruns")
            self.assertEqual(check["status"], "pass")

    def test_artifact_manifest_hashes_files_as_streams(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            artifact = run / "large.bin"
            artifact.write_bytes(b"Beach Bounce" * 1000)
            with mock.patch.object(Path, "read_bytes", side_effect=AssertionError("whole-file read")):
                manifest = observability.artifact_manifest(run)
            self.assertEqual(manifest["artifacts"][0]["bytes"], artifact.stat().st_size)
            self.assertEqual(len(manifest["artifacts"][0]["sha256"]), 64)


    def test_healthy_30_fps_uses_scenario_target_for_actual_frames(self):
        for location in ("acceptance", "scenario"):
            for key in ("target_fps", "expected_fps"):
                with self.subTest(location=location, key=key), tempfile.TemporaryDirectory() as d:
                    run = Path(d)
                    metadata = {"acceptance": {"gameplay_window": [10, 20]}}
                    (metadata["acceptance"] if location == "acceptance" else metadata)[key] = 30
                    (run / "scenario.json").write_text(json.dumps(metadata))
                    (run / "run.log").write_text(
                        "[ 20.0] [vk] pacing: 30 FPS | p95 33.34 p99 33.34 max 33.34 ms | "
                        ">16.67 300 >33/50/100 300/0/0 (last 300 intervals)\n")
                    (run / "frames-1.csv").write_text(
                        "#present,guest_s,frame_ms\n" + "".join(
                            f"{i},{10 + i / 30},{1000 / 30}\n" for i in range(1, 301)))
                    summary = observability.generate(run, {"exit": 0, "why": "exited", "seconds": 20})
                    perf = summary["performance"]
                    self.assertEqual(perf["source"], "frames")
                    self.assertEqual(perf["expected_fps"], 30)
                    self.assertAlmostEqual(perf["expected_frame_interval_ms"], 1000 / 30)
                    self.assertEqual((perf["fps"], perf["late_frames"], perf["dropped_frames"]), (30, 0, 0))
                    self.assertEqual(perf["pacing"]["over_16_67_ms"], 300)
                    self.assertEqual(summary["verdict"]["status"], "pass")

    def test_60_fps_missing_frame_and_startup_exclusion(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {
                "target_fps": 60, "gameplay_window": [10, 20], "max_stutters": 0}}))
            (run / "frames-1.csv").write_text(
                "#present,guest_s,frame_ms\n1,1,1000\n" +
                f"2,10,{1000 / 60}\n3,15,{2000 / 60}\n4,20,{1000 / 60}\n")
            summary = observability.generate(run)
            perf = summary["performance"]
            self.assertEqual(perf["frames"], 3)
            self.assertEqual(perf["late_frames"], 1)
            self.assertEqual(perf["dropped_frames"], 1)
            self.assertEqual(perf["stutters"], 0)
            self.assertEqual(observability._frame_summary(run)["stutters"], 1)

    def test_presentmon_uses_30_fps_target(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {"expected_fps": 30}}))
            (run / "presentmon.csv").write_text(
                f"TimeInSeconds,MsBetweenPresents\n1,{1000 / 30}\n2,{2000 / 30}\n")
            perf = observability.generate(run)["performance"]
            self.assertEqual(perf["source"], "presentmon")
            self.assertEqual((perf["late_frames"], perf["dropped_frames"]), (1, 1))

    def test_30_fps_pacing_buckets_cannot_establish_drops_or_lateness(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {"target_fps": 30}}))
            (run / "run.log").write_text(
                "[ 15.0] [vk] pacing: 30 FPS | p95 33.34 p99 33.34 max 33.34 ms | "
                ">16.67 150 >33/50/100 150/0/0 (last 150 intervals)\n")
            perf = observability.generate(run)["performance"]
            self.assertEqual(perf["source"], "pacing_log_estimate")
            self.assertEqual(perf["expected_fps"], 30)
            self.assertIsNone(perf["dropped_frames"])
            self.assertIsNone(perf["late_frames"])
            self.assertEqual(perf["stutters"], 0)
            self.assertEqual(perf["pacing"]["over_33_50_100_ms"], [150, 0, 0])
            self.assertNotIn("dropped_frames_estimate", perf["pacing"])
            (run / "run.log").write_text(
                "[ 15.0] [vk] pacing: 30 FPS | p95 33.34 p99 33.34 max 33.34 ms\n")
            self.assertIsNone(observability.generate(run)["performance"]["stutters"])

    def test_innocuous_shader_rewrites_are_not_refusals(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {}}))
            (run / "run.log").write_text(
                "[ 1.0] [vk] shader abc: 2 subgroup ballot(s) for a counter made per lane (RTX 50 hang)\n"
                "[vk] shader def: transform feedback variant (2 outputs into 1 buffers, strides [16])\n"
                "[vk] shader 123 is not in the shader library\n"
                "[vk] prewarm: 4 of 10 pipelines in 0.25 s (6 without their shaders), private memory +1 MB\n")
            summary = observability.generate(run, {"exit": 0, "why": "exited"})
            shaders = summary["shaders"]
            self.assertEqual(shaders["refused_unique"], 0)
            self.assertEqual(shaders["missing_keys"], ["123"])
            self.assertEqual(shaders["prewarm"], [{"built": 4, "keys": 10, "without_shaders": 6}])
            self.assertEqual(summary["verdict"]["status"], "pass")
            with (run / "run.log").open("a") as log:
                log.write("[RED] shader dead: unsupported module\n"
                          "[RED] shader dead: unsupported module\n"
                          "[vk] shader beef: module creation failed\n")
            summary = observability.generate(run, {"exit": 0, "why": "exited"})
            self.assertEqual(summary["shaders"]["refused_keys"], ["beef", "dead"])
            self.assertEqual(summary["verdict"]["status"], "fail")

    def test_nonzero_exits_fail_even_with_good_fps_and_full_duration(self):
        for exit_code in (1, 70, 71):
            with self.subTest(exit=exit_code), tempfile.TemporaryDirectory() as d:
                run = Path(d)
                (run / "scenario.json").write_text(json.dumps({"acceptance": {
                    "target_fps": 30, "gameplay_window": [10, 20]}}))
                (run / "run.log").write_text("[vk] normal rendering\n")
                (run / "frames-1.csv").write_text(
                    "#present,guest_s,frame_ms\n1,10,33.333\n2,20,33.333\n")
                (run / "result.json").write_text(json.dumps({"exit": exit_code, "why": "exited", "seconds": 20}))
                summary = observability.generate(run)
                checks = {c["name"]: c for c in summary["verdict"]["checks"]}
                self.assertEqual(checks["performance.expected_fps"]["status"], "pass")
                self.assertEqual(checks["lifecycle.observed_gameplay_seconds"]["status"], "pass")
                self.assertEqual(checks["lifecycle.exit_code"]["status"], "fail")
                self.assertEqual(summary["verdict"]["status"], "fail")
                self.assertFalse(summary["verdict"]["pass"])

    def test_short_observation_fails_and_partial_diagnostics_remain_unknown(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {
                "target_fps": 30, "gameplay_window": [10, 20]}}))
            (run / "run.log").write_text("[vk] normal rendering\n")
            (run / "frames-1.csv").write_text(
                "#present,guest_s,frame_ms\n1,10,33.333\n2,12,33.333\n")
            # A long process runtime and a named screenshot do not establish observed gameplay duration.
            write_png(run / "gameplay.png", 1, 1, [[(20, 200, 20)]])
            (run / "actions.jsonl").write_text(json.dumps({
                "screenshot": "gameplay.png", "name": "gameplay-complete", "elapsed": 20}) + "\n")
            summary = observability.generate(run, {"exit": 0, "why": "exited", "seconds": 100})
            checks = {c["name"]: c for c in summary["verdict"]["checks"]}
            self.assertEqual(checks["performance.expected_fps"]["status"], "pass")
            self.assertEqual(checks["lifecycle.observed_gameplay_seconds"]["actual"], 2)
            self.assertEqual(summary["verdict"]["status"], "fail")
            self.assertEqual(observability.generate(run)["verdict"]["status"], "unknown")

            # Startup-only telemetry must not satisfy a later gameplay window.
            (run / "frames-1.csv").write_text(
                "#present,guest_s,frame_ms\n1,1,33.333\n2,5,33.333\n3,30,33.333\n")
            summary = observability.generate(run, {"exit": 0, "why": "exited", "seconds": 100})
            check = next(c for c in summary["verdict"]["checks"]
                         if c["name"] == "lifecycle.observed_gameplay_seconds")
            self.assertEqual((check["actual"], check["status"]), (0, "fail"))

    def test_runtime_and_analysis_window_cannot_bypass_required_duration(self):
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            (run / "scenario.json").write_text(json.dumps({"acceptance": {
                "target_fps": 30, "gameplay_window": [10, 20]}}))
            (run / "run.log").write_text("[vk] normal rendering\n")
            (run / "frames-1.csv").write_text(
                "#present,guest_s,frame_ms\n1,10,33.333\n2,12,33.333\n3,20,33.333\n")
            summary = observability.generate(run, {"exit": 0, "why": "exited", "seconds": 12}, (10, 12))
            check = next(c for c in summary["verdict"]["checks"]
                         if c["name"] == "lifecycle.observed_gameplay_seconds")
            self.assertEqual(check["expected"], {"min": 10})
            self.assertEqual((check["actual"], check["status"]), (2, "fail"))
            self.assertFalse(summary["verdict"]["pass"])

    def test_generated_summaries_match_schema(self):
        try:
            from jsonschema import Draft202012Validator
        except ImportError:
            self.skipTest("jsonschema is not installed")
        schema = json.loads(Path(__file__).with_name("run-summary.schema.json").read_text())
        Draft202012Validator.check_schema(schema)
        validator = Draft202012Validator(schema)
        with tempfile.TemporaryDirectory() as d:
            run = Path(d)
            validator.validate(observability.generate(run))
            (run / "scenario.json").write_text(json.dumps({"acceptance": {
                "target_fps": 30, "gameplay_window": [10, 20]}}))
            (run / "run.log").write_text(
                "[ 15.0] [vk] pacing: 30 FPS | p95 33.34 p99 33.34 max 33.34 ms | "
                ">16.67 150 >33/50/100 150/0/0 (last 150 intervals)\n")
            validator.validate(observability.generate(run, {"exit": 71, "why": "exited", "seconds": 15}))
            (run / "frames-1.csv").write_text(
                "#present,guest_s,frame_ms\n1,10,33.333\n2,20,33.333\n")
            validator.validate(observability.generate(run, {"exit": 0, "why": "exited", "seconds": 20}))

    def test_crash_and_incomplete_termination_cannot_pass(self):
        for why, log, expected in (("crashed", "rendering", "fail"),
                                   ("exited", "thread 'render' panicked at shader.rs:1", "fail"),
                                   ("hard timeout 20 s", "rendering", "unknown")):
            with self.subTest(why=why, log=log), tempfile.TemporaryDirectory() as d:
                run = Path(d)
                (run / "scenario.json").write_text(json.dumps({"acceptance": {}}))
                (run / "run.log").write_text(log)
                summary = observability.generate(run, {"exit": 0, "why": why})
                self.assertEqual(summary["verdict"]["status"], expected)
                self.assertFalse(summary["verdict"]["pass"])


if __name__ == "__main__":
    unittest.main()
