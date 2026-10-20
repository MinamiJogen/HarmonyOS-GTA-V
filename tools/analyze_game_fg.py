#!/usr/bin/env python3
"""Report game FG facts from status, RS flush:present records and optional P6 captures.

Uses only the Python standard library. There is deliberately no acceptance/pass
decision. RS records are associated only by inclusive CLOCK_MONOTONIC present
call intervals, never by nearest timestamps, clock offsets or target frame rates.
"""

import argparse
from collections import Counter, defaultdict
import hashlib
import json
import math
from pathlib import Path
import re
import sys


PAIR = re.compile(r"^\s*(\d+)\s*:\s*(\d+)\s*$")
SURFACE = re.compile(r"^\s*surface\s+\[(.*?)\]\s*:\s*$", re.IGNORECASE)
PPM_LIMIT = 64 * 1024 * 1024
COUNTERS = (
    "guestFinalFramesSeen", "selectedSourceFrames", "mappedSourceFrames",
    "aiCompleted", "aiResultsReceived", "sourcePresents", "aiPresents",
    "repeatPresents", "missedDeadlineSlots", "underrunSlots", "lateAiDiscarded",
    "outputSlotsSkipped", "surfaceAcquireFailures", "historyResets",
    "readbackBackpressureDrops", "workerBackpressureDrops",
    "workerResultBackpressureDrops", "workerStaleInferenceResults",
    "eventCapacityDrops", "qualityGateRejectedMidpoints", "gpuWarpEncoded",
    "orphanFlowResults",
)


def integer(value):
    return isinstance(value, int) and not isinstance(value, bool)


def finite_number(value):
    return (isinstance(value, (int, float)) and not isinstance(value, bool)
            and math.isfinite(value))


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def load_json(path):
    raw = path.read_bytes()
    value = json.loads(raw, parse_constant=lambda text: (_ for _ in ()).throw(
        ValueError(f"non-finite JSON number: {text}")))
    if not isinstance(value, dict):
        raise ValueError(f"expected JSON object: {path}")
    return value, {"path": str(path.resolve()), "bytes": len(raw), "sha256": sha256(raw)}


def distribution(values):
    """Nearest-rank percentiles; do not round measurements to target values."""
    values = sorted(values)
    if not values:
        return {"count": 0, "min": None, "p50": None, "p95": None, "max": None}
    return {
        "count": len(values), "min": values[0],
        "p50": values[max(0, math.ceil(len(values) * .50) - 1)],
        "p95": values[max(0, math.ceil(len(values) * .95) - 1)],
        "max": values[-1],
    }


def cadence(times, units_per_second):
    """n-1 intervals over last-first; repeated timestamps remain visible."""
    ordered = sorted(times)
    span = ordered[-1] - ordered[0] if ordered else None
    intervals = [b - a for a, b in zip(ordered, ordered[1:])]
    return {
        "records": len(ordered), "firstTimestamp": ordered[0] if ordered else None,
        "lastTimestamp": ordered[-1] if ordered else None,
        "spanSeconds": span / units_per_second if span is not None else None,
        "observedHz": ((len(ordered) - 1) * units_per_second / span
                       if len(ordered) > 1 and span > 0 else None),
        "intervalMs": distribution([v * 1000 / units_per_second for v in intervals]),
        "zeroIntervals": intervals.count(0),
        "intervalsOver50Ms": sum(v * 1000 > 50 * units_per_second for v in intervals),
        "intervalsOver66_666667Ms": sum(v * 1000 > (200 / 3) * units_per_second for v in intervals),
        "rateFormula": "(record_count - 1) / (last_timestamp - first_timestamp)",
        "scope": "Record cadence; equal fence timestamps are exposed and do not prove distinct scanouts.",
    }


def kinds(rows):
    result = {"source": 0, "ai": 0, "repeat": 0}
    for row in rows:
        key = row.get("kind")
        key = key if isinstance(key, str) else "unknown"
        result[key] = result.get(key, 0) + 1
    return result


def slots(rows):
    values = [row["slot"] for row in rows if integer(row.get("slot"))]
    deltas = [b - a for a, b in zip(values, values[1:])]
    return {
        "rowsWithSlot": len(values), "firstSlot": values[0] if values else None,
        "lastSlot": values[-1] if values else None,
        "positiveGapCount": sum(delta > 1 for delta in deltas),
        "missingSlotsBetweenRows": sum(max(0, delta - 1) for delta in deltas),
        "duplicateAdjacentSlots": deltas.count(0),
        "backwardAdjacentSlots": sum(delta < 0 for delta in deltas),
        "scope": "Only gaps between retained rows; matched-RS gaps may include records outside RS coverage, not just app skips.",
    }


def status_facts(status):
    raw = status.get("recentPresents", [])
    if not isinstance(raw, list):
        raise ValueError("status.recentPresents must be an array")
    rows = [row for row in raw if isinstance(row, dict)]
    called = [row for row in rows if row.get("nativePresentCalled") is True]
    host = [row["hostElapsedMs"] for row in called if finite_number(row.get("hostElapsedMs"))]
    all_host = [row["hostElapsedMs"] for row in rows if finite_number(row.get("hostElapsedMs"))]
    return rows, {
        "pid": status.get("pid"), "epoch": status.get("epoch"),
        "status": status.get("status"), "error": status.get("error"),
        "timestampClock": status.get("timestampClock"),
        "recentRows": {
            "records": len(rows), "nonObjectRecords": len(raw) - len(rows),
            "kindCountsAllRows": kinds(rows), "nativePresentCalls": len(called),
            "kindCountsNativePresentCalls": kinds(called),
            "acquireFailureRows": sum(row.get("nativePresentCalled") is False for row in rows),
            "rowsWithoutNativePresentFlag": sum(not isinstance(row.get("nativePresentCalled"), bool) for row in rows),
            "hostCallCadence": cadence(host, 1000),
            "hostTimeBackwardTransitions": sum(b < a for a, b in zip(all_host, all_host[1:])),
            "allRowSlots": slots(rows), "nativePresentCallSlots": slots(called),
            "scope": "Bounded recent trace; host queue.present call rate is not compositor/display FPS. First preceding interval and earlier skips are outside this trace.",
        },
        "dimensionsAndPipeline": {key: status.get(key) for key in (
            "guestFinalSource", "nativeBufferWidth", "nativeBufferHeight", "physicalWindow",
            "inferenceWidth", "inferenceHeight", "modelReadbackWidth", "modelReadbackHeight",
            "bridgeRgbInputWidth", "bridgeRgbInputHeight", "spatialUpscaling", "gpuWarpEnabled",
            "cpuWarpEnabled", "backendImagePath", "pipeline", "normalization", "maskStorage")},
        "reportedTargetsOnly": {key: status.get(key) for key in (
            "targetSourceHz", "targetPresentationHz", "bufferedDelayMs",
            "configuredOutputFps", "outputPeriodNs", "sourcePeriodNs", "clockConfiguration")},
        "cumulativeCountersFromStatus": {key: status[key] for key in COUNTERS if key in status},
        "reportedCumulativeHostCallHz": status.get("nativePresentCallsPerElapsedSecond"),
        "reportedInference": {key: status.get(key) for key in (
            "recentInferenceMedianMs", "recentInferenceP95Ms", "lastNnrtRunSyncNs", "lastHalfFiniteScanNs")},
    }


def parse_rs(path):
    raw = path.read_bytes()
    groups = {}
    name = None
    zero = invalid = numeric = 0
    for line_number, line in enumerate(raw.decode("utf-8", errors="replace").splitlines(), 1):
        header = SURFACE.match(line)
        if header:
            name = header.group(1)
            groups.setdefault(name, [])
            continue
        pair = PAIR.match(line)
        if not pair:
            continue
        numeric += 1
        flush, present = map(int, pair.groups())
        if not flush or not present:
            zero += 1
        elif present < flush:
            invalid += 1
        else:
            groups.setdefault(name, []).append({"line": line_number, "flushNs": flush, "presentFenceNs": present})
    surfaces = []
    for surface, records in groups.items():
        seen, unique = set(), []
        for record in records:
            pair = (record["flushNs"], record["presentFenceNs"])
            if pair not in seen:
                unique.append(record)
                seen.add(pair)
        surfaces.append({"surfaceName": surface, "records": unique,
                         "duplicateExactPairsExcluded": len(records) - len(unique)})
    return surfaces, {
        "path": str(path.resolve()), "bytes": len(raw), "sha256": sha256(raw),
        "numericPairRows": numeric, "zeroOrPartlyZeroRowsExcluded": zero,
        "presentBeforeFlushRowsExcluded": invalid,
        "surfaceGroupCount": len(groups),
    }


def match_surface(surface, rows, clock_declared, cross_surface_rows=frozenset()):
    records = surface["records"]
    intervals, invalid = [], []
    for index, row in enumerate(rows):
        if row.get("nativePresentCalled") is not True:
            continue
        begin, end = row.get("presentBeginMonotonicNs"), row.get("presentCompleteMonotonicNs")
        if not (integer(begin) and integer(end) and 0 < begin <= end):
            invalid.append(index)
        else:
            intervals.append((index, begin, end, row))
    candidates, row_records = [], defaultdict(list)
    flush_counts = Counter(record["flushNs"] for record in records)
    for rs_index, record in enumerate(records):
        hits = [entry for entry in intervals if entry[1] <= record["flushNs"] <= entry[2]] if clock_declared else []
        candidates.append(hits)
        # Ambiguous pairs still count toward every containing row's degree.
        # Dropping them here could make another pair appear uniquely owned by
        # a row which actually contains two flushes.
        for entry in hits:
            row_records[entry[0]].append(rs_index)
    matches = []
    ambiguous_rows, ambiguous_flush, multi_rs, cross_surface = 0, 0, 0, 0
    for rs_index, (record, hits) in enumerate(zip(records, candidates)):
        if len(hits) > 1:
            ambiguous_rows += 1
            continue
        if len(hits) != 1:
            continue
        if flush_counts[record["flushNs"]] != 1:
            ambiguous_flush += 1
            continue
        index, begin, end, row = hits[0]
        if index in cross_surface_rows:
            cross_surface += 1
            continue
        if len(row_records[index]) != 1:
            multi_rs += 1
            continue
        matches.append({**record, "statusRowIndex": index, "presentBeginMonotonicNs": begin,
                        "presentCompleteMonotonicNs": end,
                        **{key: row.get(key) for key in (
                            "slot", "kind", "sourceSequence", "sourceSlot", "newerSourceSequence",
                            "newerSourceSlot", "contentEpoch", "contentKind",
                            "sourceCaptureMonotonicNs", "newerSourceCaptureMonotonicNs",
                            "trueCaptureMidpointMonotonicNs", "contentMidpointScheduledDueMonotonicNs",
                            "midpointClockQuantizationErrorNs")}})
    matches.sort(key=lambda row: (row["presentFenceNs"], row["flushNs"]))
    matched_indices = {row["statusRowIndex"] for row in matches}
    latency = {}
    for key in ("sourceCaptureMonotonicNs", "newerSourceCaptureMonotonicNs"):
        available = [row for row in matches if integer(row.get(key)) and row[key] > 0]
        valid = [row for row in available if row["presentFenceNs"] >= row[key]]
        latency[key + "ToFenceMs"] = distribution([(row["presentFenceNs"] - row[key]) / 1e6 for row in valid])
        latency[key + "AfterFenceRows"] = len(available) - len(valid)
    return {
        "surfaceName": surface["surfaceName"],
        "duplicateExactPairsExcluded": surface["duplicateExactPairsExcluded"],
        "wholeRsWindow": {
            "presentFenceCadence": cadence([row["presentFenceNs"] for row in records], 1e9),
            "flushCadence": cadence([row["flushNs"] for row in records], 1e9),
            "flushToPresentFenceMs": distribution([(row["presentFenceNs"] - row["flushNs"]) / 1e6 for row in records]),
            "scope": "All valid deduplicated RS pairs for this surface, independently of recent status coverage; no kind attribution outside exact matches.",
        },
        "matching": {
            "method": "presentBeginMonotonicNs <= RS.flushNs <= presentCompleteMonotonicNs; one RS pair per status row and one row per pair",
            "clockDeclaredExplicitMonotonic": clock_declared,
            "disabledReason": None if clock_declared else "status.timestampClock does not declare explicit CLOCK_MONOTONIC_ns; no cross-clock association attempted",
            "eligibleStatusIntervals": len(intervals), "invalidOrMissingIntervalRowIndices": invalid,
            "matchedRecords": len(matches), "matchedStatusRows": len(matched_indices),
            "rsPairsWithNoContainingInterval": sum(not hits for hits in candidates),
            "rsPairsWithOverlappingStatusIntervalsExcluded": ambiguous_rows,
            "sameFlushDifferentFencePairsExcluded": ambiguous_flush,
            "multipleRsPairsInOneStatusIntervalExcluded": multi_rs,
            "rsPairsInStatusRowsAlsoContainingOtherSurfacesExcluded": cross_surface,
            "statusIntervalsWithoutUniqueRsMatch": len(intervals) - len(matched_indices),
            "offsetAppliedNs": 0, "nearestTimestampMatchingUsed": False,
        },
        "uniqueMatchedWindow": {
            "presentFenceCadence": cadence([row["presentFenceNs"] for row in matches], 1e9),
            "kindCounts": kinds(matches), "slotGapsInFenceOrder": slots(matches),
            "sourceCaptureLatency": latency,
            "latencyScope": "Capture timestamps are host command-encoding boundaries, not GPU completion, input events or photons; includes configured buffering and RS fence latency.",
            "records": matches,
        },
    }


def read_ppm(path):
    """P6 maxval=255. Preserve raster whitespace/# rather than strip() it."""
    if path.stat().st_size > PPM_LIMIT + 4096:
        raise ValueError("PPM exceeds bounded 64 MiB raster plus header limit")
    raw = path.read_bytes()
    offset = 0

    def token():
        nonlocal offset
        while offset < len(raw):
            if offset > 4096:
                raise ValueError("PPM header exceeds 4096 bytes")
            if raw[offset] in b" \t\r\n\v\f":
                offset += 1
            elif raw[offset] == 35:
                end = raw.find(b"\n", offset)
                if end == -1:
                    raise ValueError("unterminated PPM header comment")
                offset = end + 1
            else:
                break
        start = offset
        while offset < len(raw) and raw[offset] not in b" \t\r\n\v\f#":
            offset += 1
            if offset > 4096:
                raise ValueError("PPM header exceeds 4096 bytes")
        if offset == start:
            raise ValueError("missing PPM header token")
        return raw[start:offset]

    magic, width, height, maxval = (token() for _ in range(4))
    if magic != b"P6" or not width.isdigit() or not height.isdigit() or maxval != b"255":
        raise ValueError("requires binary P6, positive decimal dimensions and maxval 255")
    width, height = int(width), int(height)
    size = width * height * 3
    if not width or not height or size > PPM_LIMIT:
        raise ValueError("PPM dimensions outside positive bounded raster limit")
    if offset >= len(raw) or raw[offset] not in b" \t\r\n\v\f":
        raise ValueError("missing single whitespace delimiter after PPM maxval")
    separator = raw[offset]
    offset += 1
    # Accept a CRLF header ending only when the exact raster length proves it.
    if separator == 13 and raw[offset:offset + 1] == b"\n" and len(raw) - offset == size + 1:
        offset += 1
    pixels = raw[offset:]
    if len(pixels) != size:
        raise ValueError(f"PPM raster length {len(pixels)} != {size}; no concatenated images/trailing data accepted")
    first = pixels[:3]
    return pixels, {
        "width": width, "height": height, "rgbBytes": len(pixels), "fileBytes": len(raw),
        "fileSha256": sha256(raw), "rgbSha256": sha256(pixels),
        "rgbByteMin": min(pixels), "rgbByteMax": max(pixels),
        "rgbByteMean": sum(pixels) / len(pixels),
        "nonzeroRgbBytes": len(pixels) - pixels.count(0),
        "firstPixelRgb": list(first),
        "spatiallyNonflat": any(pixels[i:i + 3] != first for i in range(3, len(pixels), 3)),
    }


def compare_triplet(left, middle, right):
    counts = {"leftSource": 0, "rightSource": 0, "byteBlendFloor": 0, "byteBlendHalfUp": 0}
    byte_counts = dict.fromkeys(counts, 0)
    for offset in range(0, len(middle), 3):
        differs = dict.fromkeys(counts, False)
        for channel in range(3):
            a, m, b = left[offset + channel], middle[offset + channel], right[offset + channel]
            for key, value in (("leftSource", a), ("rightSource", b),
                               ("byteBlendFloor", (a + b) // 2), ("byteBlendHalfUp", (a + b + 1) // 2)):
                if m != value:
                    byte_counts[key] += 1
                    differs[key] = True
        for key in counts:
            counts[key] += differs[key]
    return {"comparedPixels": len(middle) // 3, "differentPixels": counts, "differentRgbBytes": byte_counts}


def capture_endpoint_link(left, middle, right):
    """Join immutable actual endpoints; legacy captures used adjacent clock slots.

    Output slots express presentation chronology, not source identity. In the new
    schema an AI midpoint may have an even output slot and its real endpoints may
    span multiple source-clock buckets. A present-but-invalid explicit endpoint
    never falls back to the older adjacency assumption.
    """
    pa, pm, pb = (item.get("outputProvenance") for item in (left, middle, right))
    if not all(isinstance(item, dict) for item in (pa, pm, pb)):
        return None
    if [pa.get("kind"), pm.get("kind"), pb.get("kind")] != ["source", "ai", "source"]:
        return None
    explicit = "newerSourceSlot" in pm
    slot_values = [item.get("outputSlot") for item in (left, middle, right)]
    chronological = (all(integer(value) and value >= 0 for value in slot_values)
                     and slot_values[0] < slot_values[1] < slot_values[2])
    adjacent_outputs = (chronological
                        and slot_values[1:] == [slot_values[0] + 1, slot_values[0] + 2])
    old_sequence = (integer(pa.get("sourceSequence")) and pa["sourceSequence"] >= 0
                    and integer(pm.get("sourceSequence"))
                    and pa["sourceSequence"] == pm.get("sourceSequence"))
    old_slot = (integer(pa.get("sourceSlot")) and pa["sourceSlot"] >= 0
                and integer(pm.get("sourceSlot"))
                and pa["sourceSlot"] == pm.get("sourceSlot"))
    if explicit:
        new_slot = (integer(pm.get("newerSourceSlot")) and old_slot
                    and pm["newerSourceSlot"] > pa["sourceSlot"]
                    and integer(pb.get("sourceSlot"))
                    and pb["sourceSlot"] == pm["newerSourceSlot"])
        new_sequence = (integer(pm.get("newerSourceSequence")) and old_sequence
                        and pm["newerSourceSequence"] > pa["sourceSequence"]
                        and integer(pb.get("sourceSequence"))
                        and pb["sourceSequence"] == pm["newerSourceSequence"])
    else:
        new_slot = (old_slot and integer(pb.get("sourceSlot"))
                    and pb["sourceSlot"] == pa["sourceSlot"] + 1)
        # Some historical captures predate the explicit newer-sequence field.
        new_sequence = (integer(pb.get("sourceSequence")) and old_sequence
                        and pb["sourceSequence"] > pa["sourceSequence"]
                        and (pb["sourceSequence"] == pm["newerSourceSequence"]
                             if integer(pm.get("newerSourceSequence")) else True))
    verified = (chronological and old_sequence and old_slot and new_slot
                and new_sequence
                and (explicit or adjacent_outputs))
    return {
        "endpointMatchingMethod": ("explicit_actual_source_slot_and_sequence" if explicit
                                   else "legacy_adjacent_source_and_output_slots"),
        "legacyAdjacencyUsed": not explicit,
        "outputSlotsStrictlyChronological": chronological,
        "outputSlotsConsecutive": adjacent_outputs,
        "aiAndLeftSourceSequenceEqual": old_sequence,
        "aiAndLeftSourceSlotEqual": old_slot,
        "rightSourceSlotMatchesAiNewer": new_slot,
        "rightSourceSequenceMatchesAiNewer": new_sequence,
        "expectedNewerSourceSlot": pm.get("newerSourceSlot") if explicit else (pa["sourceSlot"] + 1 if old_slot else None),
        "expectedNewerSourceSequence": pm.get("newerSourceSequence"),
        "endpointProvenanceVerified": bool(verified),
    }


def capture_facts(directory, status):
    directory = directory.resolve()
    capture, provenance = load_json(directory / "capture-status.json")
    raw_frames = capture.get("frames", [])
    if not isinstance(raw_frames, list):
        raise ValueError("capture-status.frames must be an array")
    maps = defaultdict(list)
    output_frames = status.get("outputCaptureFrames", [])
    if not isinstance(output_frames, list):
        raise ValueError("status.outputCaptureFrames must be an array")
    for row in output_frames:
        if isinstance(row, dict) and integer(row.get("outputSlot")):
            maps[row["outputSlot"]].append(row)
    pid_equal = (capture.get("pid") == status.get("pid")
                 if integer(capture.get("pid")) and capture["pid"] > 0
                 and integer(status.get("pid")) and status["pid"] > 0 else None)
    slot_semantics = capture.get("sourceSequenceMeaning") == "output_slot_index"
    mapping_enabled = slot_semantics and pid_equal is True
    capture_slots = Counter(record.get("sourceSequence") for record in raw_frames
                            if isinstance(record, dict) and integer(record.get("sourceSequence")))
    frames, usable = [], []
    for index, record in enumerate(raw_frames):
        if not isinstance(record, dict):
            frames.append({"recordIndex": index, "error": "capture record is not an object"})
            continue
        image = record.get("image") if isinstance(record.get("image"), dict) else {}
        filename = image.get("filename")
        slot = record.get("sourceSequence") if slot_semantics else None
        mapping = maps.get(slot, []) if integer(slot) and mapping_enabled else []
        result = {
            "recordIndex": index, "captureSequence": record.get("captureSequence"),
            "captureRecordSourceSequence": record.get("sourceSequence"),
            "captureStatus": record.get("status"), "filename": filename,
            "reportedWidth": record.get("width"), "reportedHeight": record.get("height"),
            "readbackGuardIntact": image.get("readbackGuardIntact"),
            "rowPaddingGuardIntact": image.get("rowPaddingGuardIntact"),
            "outputSlot": slot, "mappingCandidates": len(mapping),
            "captureRecordsForSameOutputSlot": capture_slots.get(slot, 0) if integer(slot) else None,
            "outputProvenance": mapping[0] if len(mapping) == 1 and capture_slots.get(slot) == 1 else None,
        }
        try:
            if not isinstance(filename, str):
                raise ValueError("saved PPM filename absent")
            path = (directory / filename).resolve()
            if not path.is_relative_to(directory) or path == directory or path.suffix.lower() != ".ppm":
                raise ValueError("PPM filename must resolve inside capture directory")
            _, facts = read_ppm(path)
            result.update(facts)
            result["reportedDimensionsEqualPpm"] = (facts["width"], facts["height"]) == (record.get("width"), record.get("height"))
            reported_hash = image.get("rgbSha256")
            result["reportedRgbSha256Equal"] = facts["rgbSha256"] == reported_hash if isinstance(reported_hash, str) else None
            usable.append((result, path))
        except (OSError, ValueError) as error:
            result["error"] = str(error)
        frames.append(result)
    triplets, links = [], []
    for mid_index, mid in enumerate(usable):
        m = mid[0]
        pm = m.get("outputProvenance")
        if not isinstance(pm, dict) or pm.get("kind") != "ai":
            continue
        candidates = []
        for first in usable[:mid_index]:
            for last in usable[mid_index + 1:]:
                linked = capture_endpoint_link(first[0], m, last[0])
                if linked and linked["endpointProvenanceVerified"]:
                    candidates.append((first, last, linked))
        links.append({"aiCaptureSequence": m["captureSequence"], "aiOutputSlot": m["outputSlot"],
                      "endpointMatchingMethod": ("explicit_actual_source_slot_and_sequence"
                                                 if "newerSourceSlot" in pm else "legacy_adjacent_source_and_output_slots"),
                      "verifiedEndpointPairCandidates": len(candidates),
                      "error": (None if len(candidates) == 1 else
                                "no unique captured source endpoints; no endpoint/blend comparison attributed")})
        if len(candidates) != 1:
            continue
        first, last, linked = candidates[0]
        a, b = first[0], last[0]
        pa, pb = a["outputProvenance"], b["outputProvenance"]
        result = {"captureSequences": [item["captureSequence"] for item in (a, m, b)],
                  "captureRecordIndices": [item["recordIndex"] for item in (a, m, b)],
                  "outputSlots": [item["outputSlot"] for item in (a, m, b)],
                  "sourceSequences": [item.get("sourceSequence") for item in (pa, pm, pb)],
                  "sourceSlots": [item.get("sourceSlot") for item in (pa, pm, pb)], **linked}
        if len({(item["width"], item["height"]) for item in (a, m, b)}) != 1:
            result["error"] = "triplet PPM dimensions differ"
        else:
            result.update(compare_triplet(*(read_ppm(item[1])[0] for item in (first, mid, last))))
        triplets.append(result)
    return {
        "input": provenance, "pid": capture.get("pid"),
        "pidEqualsStatus": pid_equal,
        "sourceBoundary": capture.get("sourceBoundary"),
        "sourceSequenceMeaning": capture.get("sourceSequenceMeaning"),
        "outputSlotMappingEnabled": mapping_enabled,
        "captureStatus": capture.get("status"), "reportedErrors": capture.get("errors"),
        "savedFramesReported": capture.get("savedFrames"), "frames": frames,
        "sourceAiEndpointLinks": links,
        "sourceAiSourceTriplets": triplets,
        "consecutiveSourceAiSourceTriplets": [item for item in triplets
            if item["captureRecordIndices"][1:] == [item["captureRecordIndices"][0] + 1,
                                                   item["captureRecordIndices"][0] + 2]],
        "scope": "PPM facts at the labeled capture boundary, before native present. Only uniquely linked captured source/AI/source endpoints are compared; missing endpoints are not synthesized. Explicit newer source slots need not be adjacent and AI output slots may be even. Differences from endpoints or encoded-byte 50% blends are not ground-truth interpolation or SR quality acceptance. Capture boundaryMonotonicNs is capture-local elapsed time and is never joined to RS.",
    }


def analyze(status_path, rs_path, capture_directory=None):
    status, provenance = load_json(status_path)
    rows, facts = status_facts(status)
    surfaces, rs_input = parse_rs(rs_path)
    clock = status.get("timestampClock")
    declared = isinstance(clock, str) and clock.startswith("CLOCK_MONOTONIC_ns")
    # Do not attribute another window's coincident flush to a game row. Even
    # separate per-surface reports must agree on a unique window association.
    row_surfaces = defaultdict(set)
    if declared and len(surfaces) > 1:
        for surface_index, surface in enumerate(surfaces):
            for index, row in enumerate(rows):
                begin, end = row.get("presentBeginMonotonicNs"), row.get("presentCompleteMonotonicNs")
                if row.get("nativePresentCalled") is True and integer(begin) and integer(end) and 0 < begin <= end:
                    if any(begin <= record["flushNs"] <= end for record in surface["records"]):
                        row_surfaces[index].add(surface_index)
    cross_surface_rows = {index for index, names in row_surfaces.items() if len(names) > 1}
    report = {
        "schemaVersion": 1, "tool": "tools/analyze_game_fg.py",
        "toolSha256": sha256(Path(__file__).read_bytes()),
        "inputs": {"status": provenance, "renderService": rs_input},
        "scope": "Facts only; no required-pass or global acceptance decision. Target rates, dimensions and encoded-command flags never establish >=30 display FPS, AI quality, SR quality or simultaneous 1080p30 acceptance. Rates retain precision without rounding to a target.",
        "percentileMethod": "nearest rank: sorted[ceil(n*p)-1]",
        "status": facts,
        "renderService": {"surfaceCount": len(surfaces),
                          "multipleSurfaceScope": "Each surface is analyzed separately; rates and kind counts are never pooled across surfaces.",
                          "surfaces": [match_surface(surface, rows, declared, cross_surface_rows) for surface in surfaces]},
        "capture": capture_facts(capture_directory, status) if capture_directory else None,
    }
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--status", required=True, type=Path)
    parser.add_argument("--rs", required=True, type=Path)
    parser.add_argument("--capture", type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args(argv)
    try:
        sources = [args.status.resolve(), args.rs.resolve(), Path(__file__).resolve()]
        if args.capture:
            sources.append((args.capture / "capture-status.json").resolve())
        if args.out.resolve() in sources:
            raise ValueError("--out must not overwrite an input")
        report = analyze(args.status, args.rs, args.capture)
        if args.capture:
            for frame in report["capture"]["frames"]:
                if isinstance(frame.get("filename"), str) and args.out.resolve() == (args.capture / frame["filename"]).resolve():
                    raise ValueError("--out must not overwrite a capture image")
        args.out.parent.mkdir(parents=True, exist_ok=True)
        args.out.write_text(json.dumps(report, indent=2, ensure_ascii=False, allow_nan=False) + "\n", encoding="utf-8")
    except (OSError, ValueError, TypeError) as error:
        print(f"analyze_game_fg: {error}", file=sys.stderr)
        return 2
    print(json.dumps({"out": str(args.out.resolve()), "surfaces": report["renderService"]["surfaceCount"],
                      "uniqueMatches": [surface["matching"]["matchedRecords"] for surface in report["renderService"]["surfaces"]]}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
