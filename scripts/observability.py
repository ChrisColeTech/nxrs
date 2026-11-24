"""Run artifact collection and machine-readable diagnostic summaries.

The parser deliberately accepts partially written and older CSV/log formats. A run that
crashes or is killed should still get a useful summary rather than lose all diagnostics.
"""
import csv
import hashlib
import json
import math
import os
import re
import struct
import time
import zlib
from pathlib import Path
import run_triage

SCHEMA = "nxrs.run-summary.v1"
STUTTER_MS = 50.0
RED_CATEGORIES = (
    "shader_resource_binding",
    "pipeline_crash_blacklist_compiler",
    "validation",
    "device_gpu",
    "service_gap",
    "other",
)


def _percentile(values, q):
    if not values:
        return None
    values = sorted(values)
    at = (len(values) - 1) * q
    lo = int(math.floor(at))
    hi = int(math.ceil(at))
    return round(values[lo] + (values[hi] - values[lo]) * (at - lo), 3)


def _csv_rows(path):
    """Yield dictionaries from CSVs whose latest # line defines the header."""
    header = None
    try:
        lines = path.read_text(errors="replace").splitlines()
    except OSError:
        return
    for line in lines:
        if not line.strip():
            continue
        if line.startswith("#"):
            header = next(csv.reader([line[1:]]), None)
            continue
        fields = next(csv.reader([line]), [])
        if header is None and fields and not _number(fields[0]):
            header = fields
            continue
        if header is None and path.name.startswith("metrics-") and len(fields) >= 5:
            # Metrics files before the commented-header format used this fixed layout.
            header = ["seconds", "frames", "fps", "cpu_percent", "private_mb"]
        if header:
            yield dict(zip(header, fields))


def _number(value):
    try:
        float(value)
        return True
    except (TypeError, ValueError):
        return False


def _float(row, *names):
    for name in names:
        try:
            return float(row[name])
        except (KeyError, TypeError, ValueError):
            pass
    return None


def _int(row, *names):
    value = _float(row, *names)
    return int(value) if value is not None else 0


def _red_category(message):
    lower = message.lower()
    if any(term in lower for term in ("uniform", "storage", "texture", "sampler", "resource binding",
                                      "memory pool", "no pool", "stand-in", "stand in")):
        return "shader_resource_binding"
    if ("pipeline" in lower and any(term in lower for term in ("crash", "blacklist", "compiler", "compiling",
                                                                "not compiled"))) or "compiler crash" in lower:
        return "pipeline_crash_blacklist_compiler"
    if any(term in lower for term in ("validation", "vuid-", "validation error")):
        return "validation"
    if any(term in lower for term in ("device lost", "gpu stopped", "gpu fault", "fault address", "driver's fault",
                                      "device fault", "the gpu started", "the gpu reached",
                                      "finished everything before")):
        return "device_gpu"
    if "unimpl " in lower or re.search(r"\b(service|gap)\b", lower):
        return "service_gap"
    return "other"


def _red_diagnostics(text):
    grouped = {category: {} for category in RED_CATEGORIES}
    total_lines = 0
    for line in text.splitlines():
        marker = line.find("[RED]")
        if marker < 0:
            continue
        total_lines += 1
        timestamp = re.match(r"^\[(?:os\s+)?\s*([\d.]+)\]", line, re.I)
        message = line[marker + len("[RED]"):].strip()
        # Addresses and offsets are allocation-specific. Shader keys and decimal resource IDs are stable evidence.
        normalized = re.sub(r"0x[0-9a-f]+", "<hex>", message, flags=re.I)
        normalized = re.sub(r"\s+", " ", normalized).strip()
        category = _red_category(message)
        cross_reference = []
        lower = message.lower()
        if "device lost" in lower or "gpu stopped responding" in lower:
            cross_reference.append("gpu.device_lost")
        if "fault address:" in lower or "driver's fault report:" in lower:
            cross_reference.append("gpu.faults")
        if any(term in lower for term in ("the gpu started", "the gpu reached", "finished everything before")):
            cross_reference.append("gpu.checkpoints")
        if re.search(r"(?:^|\] )shader [0-9a-f]+:", message, re.I):
            cross_reference.append("shaders.refused_unique")
        if "unimpl " in lower:
            cross_reference.append("gaps.items")
        item = grouped[category].get(normalized)
        if item:
            item["occurrences"] += 1
            continue
        grouped[category][normalized] = {
            "normalized": normalized,
            "representative": message,
            "first_timestamp": float(timestamp.group(1)) if timestamp else None,
            "occurrences": 1,
            "cross_references": cross_reference,
        }
    categories = {}
    all_items = []
    for category in RED_CATEGORIES:
        items = list(grouped[category].values())
        all_items.extend(items)
        categories[category] = {
            "count": len(items),
            "occurrences": sum(item["occurrences"] for item in items),
            "items": items,
        }
    return {
        "total_lines": total_lines,
        "unique": len(all_items),
        "cross_referenced_unique": sum(bool(item["cross_references"]) for item in all_items),
        "categories": categories,
    }


def _unallowed_red_diagnostics(red, acceptance=None):
    acceptance = acceptance or {}
    allowed_categories = acceptance.get("allowed_red_categories", [])
    if isinstance(allowed_categories, str):
        allowed_categories = [allowed_categories]
    allowed_categories = set(allowed_categories)
    patterns = acceptance.get("allowed_red_patterns", [])
    if isinstance(patterns, str):
        patterns = [patterns]
    compiled = []
    for pattern in patterns:
        try:
            compiled.append(re.compile(pattern, re.I))
        except (TypeError, re.error):
            continue
    unallowed = []
    for category, group in red["categories"].items():
        for item in group["items"]:
            if item["cross_references"] or category in allowed_categories:
                continue
            if any(pattern.search(item["representative"]) or pattern.search(item["normalized"])
                   for pattern in compiled):
                continue
            unallowed.append(item)
    return unallowed


def _in_window(value, window):
    return not window or value is not None and window[0] <= value <= window[1]


def _expected_fps(metadata):
    acceptance = metadata.get("acceptance") if metadata else None
    for settings in (acceptance, metadata):
        if isinstance(settings, dict):
            for key in ("expected_fps", "target_fps"):
                value = _float(settings, key)
                if value is not None and math.isfinite(value) and value > 0:
                    return value
    return 60.0  # Legacy scenarios did not declare a target.


def _frame_summary(run, window=None, pacing=None, expected_fps=60.0):
    intervals = []
    dropped = 0
    expected_ms = 1000.0 / expected_fps
    target = {"expected_fps": expected_fps, "expected_frame_interval_ms": expected_ms}
    for path in sorted(run.glob("frames-*.csv")):
        for row in _csv_rows(path):
            ms = _float(row, "frame_ms")
            when = _float(row, "guest_s", "seconds", "TimeInSeconds", "CPUStartTime")
            if ms is not None and ms > 0 and _in_window(when, window):
                intervals.append(ms)
                dropped += _int(row, "draws_skipped", "dropped")
    source = "frames"
    if not intervals:
        pm = run / "presentmon.csv"
        for row in _csv_rows(pm) if pm.exists() else ():
            ms = _float(row, "MsBetweenPresents", "FrameTime", "msBetweenPresents")
            when = _float(row, "TimeInSeconds", "CPUStartTime", "seconds")
            if ms is not None and ms > 0 and _in_window(when, window):
                intervals.append(ms)
        source = "presentmon" if intervals else None
    total_ms = sum(intervals)
    if not intervals and pacing and pacing["samples"]:
        return {
            **target,
            "source": "pacing_log_estimate",
            "frames": pacing["intervals"],
            "fps": pacing["fps_mean"],
            "frame_time_ms": {"p50": None, "p95": pacing["p95_ms"], "p99": pacing["p99_ms"],
                              "max": pacing["max_ms"]},
            "late_frames": None,
            "late_frames_method": "unknown: fixed pacing buckets cannot resolve target-relative lateness",
            "dropped_frames": None,
            "dropped_frames_method": "unknown: fixed pacing buckets cannot resolve target-relative drops",
            "stutters": pacing["stutters_estimate"],
            "stutters_method": "estimated from pacing >50 ms bucket",
            "stutter_threshold_ms": STUTTER_MS,
            "dropped_work": 0,
        }
    return {
        **target,
        "source": source,
        "frames": len(intervals),
        "fps": round(1000.0 * len(intervals) / total_ms, 3) if total_ms else None,
        "frame_time_ms": {
            "p50": _percentile(intervals, .50),
            "p95": _percentile(intervals, .95),
            "p99": _percentile(intervals, .99),
            "max": round(max(intervals), 3) if intervals else None,
        },
        "late_frames": sum(v > expected_ms for v in intervals) if intervals else None,
        "late_frames_method": "counted frame intervals > expected frame interval",
        "dropped_frames": sum(max(0, int(round(v / expected_ms)) - 1) for v in intervals) if intervals else None,
        "dropped_frames_method": "estimated missing target-rate presentation intervals; not measured drops",
        "stutters_method": "counted frame intervals >= 50 ms",
        "stutters": sum(v >= STUTTER_MS for v in intervals),
        "stutter_threshold_ms": STUTTER_MS,
        "dropped_work": dropped,
    }


def _metrics_fps(run, window=None):
    values = []
    for path in sorted(run.glob("metrics-*.csv")):
        for row in _csv_rows(path):
            when = _float(row, "seconds")
            fps = _float(row, "fps")
            if fps is not None and _in_window(when, window):
                values.append(fps)
    return {
        "samples": len(values),
        "mean": round(sum(values) / len(values), 3) if values else None,
        "min": round(min(values), 3) if values else None,
    }


def _metrics_summary(run, window=None):
    samples = []
    totals = {}
    for path in sorted(run.glob("metrics-*.csv")):
        for row in _csv_rows(path):
            sec = _float(row, "seconds")
            mem = _float(row, "private_mb")
            if sec is not None and mem is not None and _in_window(sec, window):
                samples.append((sec, mem))
            for key, value in row.items():
                if key not in ("seconds", "frames", "fps", "cpu_percent", "private_mb", "xmon") and _number(value):
                    totals[key] = totals.get(key, 0) + int(float(value))
    harness = run / "harness-memory.csv"
    if harness.exists():
        # This clock stays continuous across an executable hand-over; engine metrics restart at zero in its successor.
        harness_samples = []
        for row in _csv_rows(harness):
            sec, mem = _float(row, "seconds"), _float(row, "private_mb")
            # A terminal zero is emitted after the watched process exits; it is not a memory sample.
            if sec is not None and mem is not None and mem > 0 and _in_window(sec, window):
                harness_samples.append((sec, mem))
        if harness_samples:
            samples = harness_samples
    samples.sort()
    slope = None
    if len(samples) >= 2 and samples[-1][0] > samples[0][0]:
        xs, ys = zip(*samples)
        xm, ym = sum(xs) / len(xs), sum(ys) / len(ys)
        den = sum((x - xm) ** 2 for x in xs)
        slope = 60.0 * sum((x - xm) * (y - ym) for x, y in samples) / den if den else 0.0
    growth = samples[-1][1] - samples[0][1] if len(samples) >= 2 else 0.0
    memory = {
        "samples": len(samples),
        "start_mb": round(samples[0][1], 1) if samples else None,
        "peak_mb": round(max((m for _, m in samples), default=0), 1) if samples else None,
        "end_mb": round(samples[-1][1], 1) if samples else None,
        "slope_mb_per_min": round(slope, 2) if slope is not None else None,
        "leak_warning": bool(slope is not None and slope >= 10.0 and growth >= 100.0),
    }
    return memory, totals


def _shader_refusals(text):
    keys = set()
    for line in text.splitlines():
        match = re.search(r"(?:^|\] )shader ([0-9a-f]+):\s*(.*)", line, re.I)
        if match and ("[RED]" in line or re.search(
                r"\b(failed|failure|refused|error)\b|\bcannot (?:build|create|load)\b",
                match.group(2), re.I)):
            keys.add(match.group(1).lower())
    return keys


def _log_summary(text, counters, window=None):
    missing = set(re.findall(r"shader ([0-9a-f]+) is not in the shader library", text, re.I))
    live_ok = set(re.findall(r"live shader ([0-9a-f]+) translated in", text, re.I))
    live_fail = set(re.findall(r"live shader ([0-9a-f]+):", text, re.I)) - live_ok
    # The same prefix is used by successful SPIR-V rewrites. Refusal needs failure evidence.
    refused = _shader_refusals(text)
    prewarm = [{"built": int(built), "keys": int(keys), "without_shaders": int(skipped)}
               for built, keys, skipped in re.findall(
                   r"prewarm: (\d+) of (\d+) pipelines in [\d.]+ s \((\d+) without their shaders\)", text)]
    gaps = {}
    for kind, what, count in re.findall(r"gap (Import|Service|Command|Svc|Nvn|Other) (.+?) x(\d+)(?:\r?$)", text, re.M):
        gaps[(kind, what)] = max(gaps.get((kind, what), 0), int(count))
    if not gaps:
        for kind, what in re.findall(r"UNIMPL (Import|Service|Command|Svc|Nvn|Other) ([^:\r\n]+)", text):
            gaps[(kind, what)] = gaps.get((kind, what), 0) + 1
    gap_summaries = [(reason, int(count)) for reason, count in re.findall(
        r"gap summary \(([^)]+)\): (\d+) distinct", text)]
    gap_types = {}
    for (kind, _), count in gaps.items():
        gap_types[kind] = gap_types.get(kind, 0) + count
    audio_streams = {}
    timed_audio = {}
    for line in text.splitlines():
        report = re.search(r"audio: ([^:\r\n]+):[^\r\n]+?, (\d+) underruns", line)
        if not report:
            continue
        name, value = report.group(1), int(report.group(2))
        audio_streams[name] = value
        timestamp = re.match(r"^\[os\s+([\d.]+)\]", line)
        if timestamp:
            timed_audio.setdefault(name, []).append((float(timestamp.group(1)), value))
    lifetime_underruns = sum(audio_streams.values()) if audio_streams else counters.get("audio_underruns", 0)
    window_streams = {}
    if window and timed_audio:
        lo, hi = window
        for name, reports in timed_audio.items():
            reports.sort()
            before = [report for report in reports if report[0] <= lo]
            inside = [report for report in reports if lo <= report[0] <= hi]
            if inside:
                baseline = before[-1][1] if before else inside[0][1]
                window_streams[name] = max(0, inside[-1][1] - baseline)
            elif before:
                window_streams[name] = 0
    window_underruns = sum(window_streams.values()) if window_streams else None
    audio_activity = {}
    for name, played, non_silent in re.findall(
            r"audio: ([^:\r\n]+): ([\d.]+) s played \(([\d.]+) s not silent\)", text):
        previous = audio_activity.get(name, (0.0, 0.0))
        audio_activity[name] = (max(previous[0], float(played)), max(previous[1], float(non_silent)))
    outputs = list(dict.fromkeys(re.findall(r"audio: output on ([^\r\n;]+)", text)))
    lost = "the GPU stopped responding (device lost" in text
    fault_lines = [line.strip() for line in text.splitlines() if "fault address:" in line or "driver's fault report:" in line]
    checkpoint_lines = [line.strip() for line in text.splitlines() if "the GPU started" in line or "the GPU reached" in line or "finished everything before" in line]
    slow = [float(v) for v in re.findall(r"\[vk\] slow frame \d+: ([\d.]+) ms", text)]
    return {
        "shaders": {
            "missing_unique": len(missing), "live_translated_unique": len(live_ok),
            "live_failed_unique": len(live_fail), "refused_unique": len(refused),
            "missing_keys": sorted(missing), "refused_keys": sorted(refused), "prewarm": prewarm,
            "pipelines_graphics": counters.get("pipelines_gfx", 0),
            "pipelines_compute": counters.get("pipelines_compute", 0),
            "compile_time_ms": round(counters.get("pipeline_ns", 0) / 1e6, 3),
        },
        "gpu": {"device_lost": lost, "faults": fault_lines, "checkpoints": checkpoint_lines},
        "gaps": {"distinct": len(gaps) if gaps else max((count for _, count in gap_summaries), default=None),
                  "hits": sum(gaps.values()) if gaps else 0 if gap_summaries else None,
                  "observed": bool(gaps or gap_summaries),
                  "summaries": [{"reason": reason, "distinct": count} for reason, count in gap_summaries],
                  "by_type": gap_types,
                  "items": [{"type": k, "name": w, "hits": n} for (k, w), n in sorted(gaps.items())]},
        "threads": {"stall_dumps": len(re.findall(r"warning: \d+ s without a new image", text)),
                    "periodic_dumps": len(re.findall(r"guest threads(?: at exit)?:", text)),
                    "dump_artifacts": []},
        "audio": {"underruns": window_underruns if window_underruns is not None else lifetime_underruns,
                  "underruns_lifetime": lifetime_underruns,
                  "underruns_gameplay_window": window_underruns,
                  "streams": audio_streams,
                  "streams_gameplay_window": window_streams,
                  "output_active": True if outputs else None,
                  "outputs": outputs,
                  "played_seconds": max((played for played, _ in audio_activity.values()), default=None),
                  "non_silent_seconds": max((active for _, active in audio_activity.values()), default=None),
                  "non_silent": any(active > 0 for _, active in audio_activity.values()) if audio_activity else None},
        "logged_slow_frames": {"count": len(slow), "max_ms": max(slow, default=None)},
        "red_diagnostics": _red_diagnostics(text),
    }


def _pacing_summary(text, window=None):
    points = [(float(t), float(fps), float(p95), float(p99), float(longest),
               int(late), int(over33), int(over50), int(over100), int(intervals))
              for t, fps, p95, p99, longest, late, over33, over50, over100, intervals in re.findall(
                  r"^\[\s*([\d.]+)\] \[vk\] pacing: ([\d.]+) FPS \| p95 ([\d.]+) p99 ([\d.]+) max ([\d.]+) ms"
                  r" \| >16\.67 (\d+) >33/50/100 (\d+)/(\d+)/(\d+) \(last (\d+) intervals\)",
                  text, re.M)]
    buckets_known = bool(points)
    if not points:
        points = [(float(t), float(fps), float(p95), float(p99), float(longest), 0, 0, 0, 0, 0)
                  for t, fps, p95, p99, longest in re.findall(
                      r"^\[\s*([\d.]+)\] \[vk\] pacing: ([\d.]+) FPS \| p95 ([\d.]+) p99 ([\d.]+) max ([\d.]+) ms",
                      text, re.M)]
    selected = [p for p in points if _in_window(p[0], window)]
    gaps = [b[0] - a[0] for a, b in zip(selected, selected[1:])]
    return {
        "samples": len(selected),
        "fps_mean": round(sum(p[1] for p in selected) / len(selected), 3) if selected else None,
        "p95_ms": max((p[2] for p in selected), default=None),
        "p99_ms": max((p[3] for p in selected), default=None),
        "max_ms": max((p[4] for p in selected), default=None),
        "max_gap_seconds": round(max(gaps), 3) if gaps else None,
        "intervals": sum(p[9] for p in selected),
        "over_16_67_ms": sum(p[5] for p in selected) if buckets_known and selected else None,
        "over_33_50_100_ms": [sum(p[i] for p in selected) for i in (6, 7, 8)]
                              if buckets_known and selected else None,
        "stutters_estimate": sum(p[7] for p in selected) if buckets_known and selected else None,
    }


def _png_pixels(data):
    """Return width, height, bytes-per-pixel and decoded rows for an 8-bit RGB(A) PNG."""
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    offset, compressed = 8, b""
    width = height = bpp = None
    while offset + 12 <= len(data):
        size, kind = struct.unpack(">I4s", data[offset:offset + 8])
        chunk = data[offset + 8:offset + 8 + size]
        if len(chunk) != size:
            return None
        if kind == b"IHDR":
            width, height, depth, colour = struct.unpack(">IIBB", chunk[:10])
            if depth != 8 or colour not in (2, 6):
                return None
            bpp = 3 if colour == 2 else 4
        elif kind == b"IDAT":
            compressed += chunk
        elif kind == b"IEND":
            break
        offset += 12 + size
    if not width or not height or not bpp:
        return None
    raw = zlib.decompress(compressed)
    stride = width * bpp
    if len(raw) != height * (stride + 1):
        return None
    previous = bytearray(stride)
    rows, offset = [], 0
    for _ in range(height):
        filter_type = raw[offset]
        if filter_type > 4:
            return None
        line = bytearray(raw[offset + 1:offset + 1 + stride])
        offset += stride + 1
        for x in range(stride):
            left = line[x - bpp] if x >= bpp else 0
            above = previous[x]
            upper_left = previous[x - bpp] if x >= bpp else 0
            if filter_type == 1:
                line[x] = (line[x] + left) & 255
            elif filter_type == 2:
                line[x] = (line[x] + above) & 255
            elif filter_type == 3:
                line[x] = (line[x] + (left + above) // 2) & 255
            elif filter_type == 4:
                estimate = left + above - upper_left
                distances = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
                line[x] = (line[x] + (left if distances[0] <= distances[1] and distances[0] <= distances[2]
                                      else above if distances[1] <= distances[2] else upper_left)) & 255
        rows.append(bytes(line))
        previous = line
    return width, height, bpp, rows


def png_pixels(path):
    return _png_pixels(Path(path).read_bytes())


def image_diagnostics(path):
    """Compute dependency-free, deterministic diagnostics for an RGB(A) PNG."""
    path = Path(path)
    try:
        data = path.read_bytes()
        decoded = _png_pixels(data)
    except (OSError, ValueError, struct.error, zlib.error) as error:
        return {"status": "unknown", "error": str(error)}
    if not decoded:
        return {"status": "unknown", "error": "not an 8-bit RGB(A) PNG"}
    width, height, bpp, rows = decoded
    step = max(1, min(width, height) // 256)
    sums = [0, 0, 0]
    black = white = purple = total = 0
    coarse = {}
    cells = [[0, 0] for _ in range(64)]
    sampled = bytearray()
    for y in range(0, height, step):
        row = rows[y]
        for x in range(0, width, step):
            red, green, blue = row[x * bpp:x * bpp + 3]
            sampled.extend((red, green, blue))
            sums[0] += red
            sums[1] += green
            sums[2] += blue
            dark = red < 16 and green < 16 and blue < 16
            black += dark
            white += red >= 240 and green >= 240 and blue >= 240
            purple += red >= 96 and blue >= 96 and red >= green * 1.25 and blue >= green * 1.25
            key = (red >> 4, green >> 4, blue >> 4)
            coarse[key] = coarse.get(key, 0) + 1
            total += 1
            cell = cells[(y * 8 // height) * 8 + x * 8 // width]
            cell[0] += dark
            cell[1] += 1
    mean = [round(value / total, 3) for value in sums]
    return {
        "status": "ok", "width": width, "height": height, "sample_step": step,
        "sampled_pixels": total, "sha256": hashlib.sha256(data).hexdigest(),
        "pixel_sha256": hashlib.sha256(sampled).hexdigest(), "mean_rgb": mean,
        "mean_luma": round(.2126 * mean[0] + .7152 * mean[1] + .0722 * mean[2], 3),
        "black_fraction": round(black / total, 6), "black_cells": sum(1 for b, n in cells if n and b == n),
        "near_white_fraction": round(white / total, 6),
        "dominant_purple_fraction": round(purple / total, 6),
        "coarse_unique_colours": len(coarse),
        "coarse_unique_fraction": round(len(coarse) / total, 6),
        "dominant_coarse_fraction": round(max(coarse.values()) / total, 6),
        "_sampled_rgb": bytes(sampled),
    }


def _screenshot_summary(run):
    milestones = {}
    actions = run / "actions.jsonl"
    if actions.exists():
        for line in actions.read_text(errors="replace").splitlines():
            try:
                action = json.loads(line)
            except json.JSONDecodeError:
                continue
            if action.get("screenshot"):
                milestones[action["screenshot"]] = action.get("name")
    screenshots, previous = [], None
    for path in sorted(run.glob("*.png")):
        diagnostic = image_diagnostics(path)
        samples = diagnostic.pop("_sampled_rgb", None)
        item = {"path": path.name, "milestone": milestones.get(path.name), **diagnostic}
        if samples is not None and previous and previous[1] == diagnostic.get("width") and previous[2] == diagnostic.get("height") and len(previous[0]) == len(samples):
            differences = [abs(a - b) for a, b in zip(samples, previous[0])]
            item["previous_frame"] = previous[3]
            item["mean_abs_rgb_difference"] = round(sum(differences) / len(differences), 6)
            item["changed_pixel_fraction"] = round(sum(any(differences[i:i + 3]) for i in range(0, len(differences), 3)) / (len(differences) // 3), 6)
        else:
            item["previous_frame"] = previous[3] if previous else None
            item["mean_abs_rgb_difference"] = None
            item["changed_pixel_fraction"] = None
        screenshots.append(item)
        if samples is not None:
            previous = (samples, diagnostic["width"], diagnostic["height"], path.name)
    return screenshots


def _scenario_metadata(run):
    path = run / "scenario.json"
    if not path.exists():
        return None, None
    try:
        metadata = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None, "scenario.json could not be read"
    if not isinstance(metadata, dict):
        return None, "scenario.json is not an object"
    acceptance = metadata.get("acceptance")
    if acceptance is not None and not isinstance(acceptance, dict):
        return metadata, "scenario acceptance metadata is not an object"
    return metadata, None


def _check(name, expected, actual, passed, evidence=None):
    status = "unknown" if actual is None else "pass" if passed else "fail"
    check = {"name": name, "status": status, "required": True, "expected": expected, "actual": actual}
    if evidence:
        check["evidence"] = evidence
    return check


def _limit_check(name, limit, actual, comparison="max", evidence=None):
    passed = actual is not None and (actual <= limit if comparison == "max" else actual >= limit)
    return _check(name, {comparison: limit}, actual, passed, evidence)


def _image_threshold_checks(thresholds, screenshots):
    if not thresholds:
        return []
    known = {shot["path"]: shot for shot in screenshots}
    known.update({shot["milestone"]: shot for shot in screenshots if shot.get("milestone")})
    if any(key in thresholds for key in ("defaults", "screenshots")):
        defaults = thresholds.get("defaults", {})
        selected = thresholds.get("screenshots", {})
    else:
        defaults, selected = thresholds, {}
    checks = []
    targets = [(shot["path"], shot, defaults) for shot in screenshots] if defaults else []
    targets += [(name, known.get(name), limits) for name, limits in sorted(selected.items())]
    definitions = {
        "max_black_fraction": ("black_fraction", "max"),
        "max_near_white_fraction": ("near_white_fraction", "max"),
        "max_dominant_purple_fraction": ("dominant_purple_fraction", "max"),
        "min_mean_luma": ("mean_luma", "min"),
        "max_mean_luma": ("mean_luma", "max"),
        "min_coarse_unique_colours": ("coarse_unique_colours", "min"),
        "max_dominant_coarse_fraction": ("dominant_coarse_fraction", "max"),
        "min_frame_difference": ("mean_abs_rgb_difference", "min"),
        "max_frame_difference": ("mean_abs_rgb_difference", "max"),
    }
    for target, shot, limits in targets:
        for threshold, limit in sorted(limits.items()):
            if threshold not in definitions:
                continue
            metric, comparison = definitions[threshold]
            actual = shot.get(metric) if shot else None
            checks.append(_limit_check("image.%s.%s" % (target, threshold), limit, actual, comparison,
                                       "screenshots[%s].%s" % (target, metric)))
    return checks


def _observed_gameplay(run, text, window):
    """Telemetry span in the configured window, not proof of gameplay content.

    Prefer actual presentation timestamps. Do not join clocks across executable
    hand-overs, use wall-clock runtime, or infer observation from screenshot labels.
    """
    sources = (
        ("frames", sorted(run.glob("frames-*.csv")), ("guest_s", "seconds", "TimeInSeconds", "CPUStartTime"),
         ("frame_ms",)),
        ("presentmon", [run / "presentmon.csv"], ("TimeInSeconds", "CPUStartTime", "seconds"),
         ("MsBetweenPresents", "FrameTime", "msBetweenPresents")),
        ("metrics", sorted(run.glob("metrics-*.csv")), ("seconds",), ("fps",)),
    )
    for source, paths, clocks, values in sources:
        spans = []
        for path in paths:
            points = []
            for row in _csv_rows(path):
                when, value = _float(row, *clocks), _float(row, *values)
                if when is not None and value is not None and math.isfinite(when) and math.isfinite(value) and value > 0:
                    start = when - value / 1000.0 if source != "metrics" else when
                    points.append((start, when))
            if points:
                inside = [p for p in points if p[1] >= window[0] and p[0] <= window[1]]
                spans.append((min(p[0] for p in inside), max(p[1] for p in inside))
                             if inside else (window[0], window[0]))
        if spans:
            return source, max(max(0.0, min(end, window[1]) - max(start, window[0]))
                               for start, end in spans)
    times = [float(t) for t in re.findall(r"^\[\s*([\d.]+)\] \[vk\] pacing:", text, re.M)]
    if times:
        inside = [t for t in times if _in_window(t, window)]
        return "pacing", max(inside) - min(inside) if inside else 0.0
    return None, None


def _lifecycle_checks(run, text, result, window):
    result = result if isinstance(result, dict) else {}
    exit_code = result.get("exit")
    checks = [_check("lifecycle.exit_code", 0, exit_code, exit_code == 0, "run.result.exit")]
    why = str(result.get("why", "")).lower()
    if (exit_code is not None and exit_code != 0 or
            re.search(r"crash|fatal|panic|watchdog|gpu reset", why) or
            re.search(r"panicked at|fatal runtime error|unhandled exception|segmentation fault", text, re.I)):
        completed = False
    elif why in ("exited", "completed") and exit_code == 0:
        completed = True
    else:
        completed = None
    checks.append(_check("lifecycle.completed", True, completed, completed is True, "run.result.why"))
    if window:
        source, observed = _observed_gameplay(run, text, window)
        required = window[1] - window[0]
        # A partial capture cannot establish completion. A finished short run is a failure.
        actual = observed if result else None
        runtime = _float(result, "seconds")
        if runtime is not None and math.isfinite(runtime):
            runtime_in_window = max(0.0, min(runtime, window[1]) - window[0])
            if runtime_in_window < required:
                actual = min(actual, runtime_in_window) if actual is not None else runtime_in_window
        check = _limit_check("lifecycle.observed_gameplay_seconds", required, actual, "min",
                             "telemetry span in configured gameplay window; not gameplay-content verification")
        check.update(observed_seconds=observed, source=source)
        checks.append(check)
    return checks


def _acceptance_summary(metadata, metadata_error, performance, metrics_fps, memory, parsed, pacing,
                        screenshots, log_exists, lifecycle):
    acceptance = metadata.get("acceptance") if metadata else None
    if metadata_error or acceptance is None:
        reason = metadata_error or "scenario.json has no acceptance metadata"
        status = "fail" if any(check["status"] == "fail" for check in lifecycle) else "unknown"
        return {"status": status, "pass": False, "reason": reason, "checks": lifecycle}

    checks = list(lifecycle)
    expected_fps = acceptance.get("expected_fps", acceptance.get("target_fps"))
    if expected_fps is None:
        expected_fps = metadata.get("expected_fps", metadata.get("target_fps"))
    if expected_fps is not None:
        actual = performance["fps"] if performance["frames"] else pacing["fps_mean"]
        tolerance = acceptance.get("fps_tolerance", acceptance.get("fps_tolerance_ratio", .98))
        expected_min = acceptance.get("min_fps", expected_fps * tolerance)
        checks.append(_limit_check("performance.expected_fps", expected_min, actual, "min",
                                   "performance.fps"))
    if "min_fps" in acceptance:
        checks.append(_limit_check("performance.min_fps", acceptance["min_fps"], metrics_fps["min"], "min",
                                   "performance.metrics_fps.min"))
    for key, alias, percentile in (("max_p95_ms", "p95_ms", "p95"), ("max_p99_ms", "p99_ms", "p99")):
        limit = acceptance.get(key, acceptance.get(alias))
        if limit is not None:
            actual = performance["frame_time_ms"][percentile]
            if actual is None:
                actual = pacing[key[4:-3] + "_ms"]
            checks.append(_limit_check("performance.%s" % key, limit, actual, "max",
                                       "performance.frame_time_ms.%s" % percentile))
    if "max_stutters" in acceptance:
        actual = performance["stutters"] if performance["source"] else None
        checks.append(_limit_check("performance.max_stutters", acceptance["max_stutters"], actual, "max",
                                    "performance.stutters"))
    if "max_frame_ms" in acceptance:
        checks.append(_limit_check("performance.max_frame_ms", acceptance["max_frame_ms"],
                                   performance["frame_time_ms"]["max"], "max", "performance.frame_time_ms.max"))
    if "allowed_gaps" in acceptance:
        checks.append(_limit_check("performance.allowed_gaps", acceptance["allowed_gaps"], pacing["max_gap_seconds"], "max",
                                   "performance.pacing.max_gap_seconds"))

    slope_limit = acceptance.get("max_memory_slope_mb_per_min", acceptance.get("memory_slope_threshold"))
    if slope_limit is not None:
        checks.append(_limit_check("memory.max_slope_mb_per_min", slope_limit, memory["slope_mb_per_min"], "max",
                                   "memory.slope_mb_per_min"))
    peak_limit = acceptance.get("max_memory_peak_mb", acceptance.get("memory_peak_threshold"))
    if peak_limit is not None:
        checks.append(_limit_check("memory.max_peak_mb", peak_limit, memory["peak_mb"] if memory["samples"] else None, "max",
                                   "memory.peak_mb"))

    checks.append(_check("gpu.no_device_loss", False, parsed["gpu"]["device_lost"] if log_exists else None,
                          log_exists and not parsed["gpu"]["device_lost"], "run.log"))
    red_limit = acceptance.get("max_red_errors", 0)
    unallowed_red = _unallowed_red_diagnostics(parsed["red_diagnostics"], acceptance)
    parsed["red_diagnostics"]["allowed_unique"] = (
        parsed["red_diagnostics"]["unique"] - parsed["red_diagnostics"]["cross_referenced_unique"] -
        len(unallowed_red)
    )
    parsed["red_diagnostics"]["unallowed_unique"] = len(unallowed_red)
    checks.append(_limit_check("diagnostics.max_red_errors", red_limit,
                               len(unallowed_red) if log_exists else None, "max", "red_diagnostics"))
    shader_fail_limit = acceptance.get("max_shader_failures", 0)
    shader_refusal_limit = acceptance.get("max_shader_refusals", 0)
    checks.append(_limit_check("shaders.max_failures", shader_fail_limit,
                               parsed["shaders"]["live_failed_unique"] if log_exists else None, "max", "run.log"))
    checks.append(_limit_check("shaders.max_refusals", shader_refusal_limit,
                               parsed["shaders"]["refused_unique"] if log_exists else None, "max", "run.log"))
    checks.append(_limit_check("audio.max_underruns", acceptance.get("max_audio_underruns", 0),
                                parsed["audio"]["underruns"] if log_exists else None, "max", "run.log"))
    if acceptance.get("require_audio_output"):
        actual = parsed["audio"]["output_active"] if log_exists else None
        checks.append(_check("audio.output_active", True, actual, actual is True, "run.log"))
    if acceptance.get("require_non_silent_audio"):
        actual = parsed["audio"]["non_silent"] if log_exists else None
        checks.append(_check("audio.non_silent", True, actual, actual is True, "run.log"))

    required = acceptance.get("required_screenshot_names", [])
    available = {shot["path"]: shot for shot in screenshots}
    available.update({shot["milestone"]: shot for shot in screenshots if shot.get("milestone")})
    for name in required:
        actual = False if name not in available else True if available[name]["status"] == "ok" else None
        checks.append(_check("screenshots.required.%s" % name, True, actual, actual is True,
                             "screenshots"))
    checks.extend(_image_threshold_checks(acceptance.get("image_thresholds"), screenshots))
    status = "fail" if any(check["status"] == "fail" for check in checks) else \
             "unknown" if not checks or any(check["status"] == "unknown" for check in checks) else "pass"
    return {"status": status, "pass": status == "pass", "checks": checks}


def _file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def artifact_manifest(run, known_hashes=None):
    known_hashes = known_hashes or {}
    entries = []
    for path in sorted(p for p in run.rglob("*") if p.is_file() and p.name not in ("summary.json", "artifact-manifest.json")):
        try:
            stat = path.stat()
            relative = path.relative_to(run).as_posix()
            digest = known_hashes.get(relative) or _file_sha256(path)
        except OSError:
            continue
        entries.append({"path": relative, "bytes": stat.st_size,
                        "sha256": digest, "modified_unix": round(stat.st_mtime, 3)})
    manifest = {"schema": "nxrs.artifact-manifest.v1", "generated_unix": round(time.time(), 3), "artifacts": entries}
    (run / "artifact-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


def generate(run, result=None, window=None):
    """Write summary.json and artifact-manifest.json for a complete or partial run."""
    run = Path(run)
    run.mkdir(parents=True, exist_ok=True)
    log = run / "run.log"
    text = log.read_text(errors="replace") if log.exists() else ""
    if result is None:
        try:
            saved_result = json.loads((run / "result.json").read_text(encoding="utf-8"))
            if isinstance(saved_result, dict):
                result = saved_result
        except (OSError, json.JSONDecodeError):
            pass
    metadata, metadata_error = _scenario_metadata(run)
    acceptance = metadata.get("acceptance") if metadata else None
    acceptance_window = acceptance.get("gameplay_window") if isinstance(acceptance, dict) else None
    effective_window = window or acceptance_window
    memory, counters = _metrics_summary(run, effective_window)
    parsed = _log_summary(text, counters, effective_window)
    if result and "GPU RESET" in str(result.get("why", "")):
        parsed["gpu"]["device_lost"] = True
    pacing = _pacing_summary(text, effective_window)
    performance = _frame_summary(run, effective_window, pacing, _expected_fps(metadata))
    metrics_fps = _metrics_fps(run, effective_window)
    screenshots = _screenshot_summary(run)
    dumps = sorted(p.name for p in run.glob("diagnostics-*-threads.txt"))
    parsed["threads"]["dump_artifacts"] = dumps
    screenshot_hashes = {shot["path"]: shot["sha256"] for shot in screenshots if shot.get("sha256")}
    run_triage.generate(run, text, effective_window)
    manifest = artifact_manifest(run, screenshot_hashes)
    lifecycle = _lifecycle_checks(run, text, result, acceptance_window)
    verdict = _acceptance_summary(metadata, metadata_error, performance, metrics_fps, memory, parsed, pacing,
                                  screenshots, bool(text.strip()), lifecycle)
    summary = {
        "$schema": "https://nxrs.dev/schemas/run-summary-v1.json",
        "schema": SCHEMA,
        "generated_unix": round(time.time(), 3),
        "run": {"result": result, "window_seconds": list(effective_window) if effective_window else None,
                "scenario": metadata},
        "performance": {**performance, "metrics_fps": metrics_fps, "pacing": pacing},
        "memory": memory,
        **parsed,
        "screenshots": screenshots,
        "verdict": verdict,
        "triage": "triage.json",
        "artifacts": {"manifest": "artifact-manifest.json", "count": len(manifest["artifacts"]),
                      "screenshots": [a["path"] for a in manifest["artifacts"] if a["path"].lower().endswith(".png")]},
    }
    (run / "summary.json").write_text(json.dumps(summary, indent=2) + "\n", encoding="utf-8")
    return summary
