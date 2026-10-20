#!/usr/bin/env python3
"""Read-only short quality-trial statistics from status and strict RS evidence."""
import argparse
import bisect
import collections
import hashlib
import json
from pathlib import Path
import re


def provenance(path):
    data = path.read_bytes()
    return {"path": str(path.resolve()), "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest()}


def analyze(directory, expected_samples, expected_pid=None, surface="muguet-game-surfaceSurface"):
    assert expected_samples >= 2, "At least two samples are required"
    collection_path = directory / "collection.json"
    collection = json.loads(collection_path.read_text())
    assert len(collection) == expected_samples, "Wait for the complete predetermined sample count"
    assert [x["sample"] for x in collection] == list(range(expected_samples))
    statuses = [json.loads((directory / f"status-{i:02}.json").read_text())
                for i in range(expected_samples)]
    identities = {(x["pid"], x["epoch"]) for x in statuses}
    assert len(identities) == 1
    pid, epoch = next(iter(identities))
    assert type(pid) is int and pid > 0
    assert expected_pid is None or pid == expected_pid
    assert all(str(x["timestampClock"]).startswith("CLOCK_MONOTONIC_ns_explicit_clock_gettime")
               for x in statuses), "Explicit device monotonic clock is required"
    assert all(x["elapsedSeconds"] < y["elapsedSeconds"] for x, y in zip(statuses, statuses[1:]))
    versions = collections.defaultdict(dict)
    for status in statuses:
        assert status["recentPresents"]
        for row in status["recentPresents"]:
            versions[(pid, epoch, row["slot"])][json.dumps(row, sort_keys=True)] = row
    assert all(len(v) == 1 for v in versions.values()), "Conflicting immutable present rows"
    rows = sorted((next(iter(v.values())) for v in versions.values()),
                  key=lambda row: row["presentBeginMonotonicNs"])
    assert all(row["nativePresentCalled"] and row["presentBeginMonotonicNs"] <= row["presentCompleteMonotonicNs"]
               and row["kind"] in ("source", "ai", "repeat") for row in rows)
    assert all(x["presentCompleteMonotonicNs"] < y["presentBeginMonotonicNs"]
               for x, y in zip(rows, rows[1:])), "Overlapping invocation intervals require an ambiguity-aware analysis"
    pairs, raw, zeros, invalid, other = set(), 0, 0, 0, 0
    for i in range(expected_samples):
        name = None
        for line in (directory / f"rs-{i:02}.txt").read_text().splitlines():
            match = re.fullmatch(r"\s*surface \[(.*)\]:\s*", line)
            if match:
                name = match[1]
                continue
            match = re.fullmatch(r"\s*(\d+):(\d+)\s*", line)
            if not match:
                continue
            if name != surface:
                other += 1
                continue
            raw += 1
            flush, fence = map(int, match.groups())
            if not flush or not fence:
                zeros += 1
            elif fence < flush:
                invalid += 1
            else:
                pairs.add((flush, fence))
    begins = [row["presentBeginMonotonicNs"] for row in rows]
    candidates, degree = [], collections.Counter()
    flush_degree = collections.Counter(flush for flush, fence in pairs)
    for flush, fence in sorted(pairs):
        i = bisect.bisect_right(begins, flush) - 1
        if i >= 0 and flush <= rows[i]["presentCompleteMonotonicNs"]:
            candidates.append((i, flush, fence))
            degree[i] += 1
    matched = [{**rows[i], "flushNs": flush, "presentFenceNs": fence}
               for i, flush, fence in candidates if degree[i] == 1 and flush_degree[flush] == 1]
    by_slot = {row["slot"]: row for row in matched}
    anchors = [by_slot.get(status["recentPresents"][-1]["slot"]) for status in statuses]
    assert all(anchors), "Every sample's latest present must have a strict RS match"
    start, end = anchors[0]["presentFenceNs"], anchors[-1]["presentFenceNs"]
    assert end > start
    arrivals = [row for row in matched if start < row["presentFenceNs"] <= end]
    counts = {kind: sum(row["kind"] == kind for row in arrivals) for kind in ("source", "ai", "repeat")}
    seconds = (end - start) / 1e9
    window_pairs = sum(start <= fence <= end for flush, fence in pairs)
    window_matched = sum(start <= row["presentFenceNs"] <= end for row in matched)
    counter_fields = ["sourcePresents", "aiPresents", "repeatPresents", "missedDeadlineSlots", "lateAiDiscarded",
                      "qualityGateRejectedMidpoints", "readbackBackpressureDrops", "workerBackpressureDrops",
                      "workerResultBackpressureDrops", "eventCapacityDrops", "surfaceAcquireFailures",
                      "historyResets", "outputSlotsSkipped", "orphanFlowResults"]
    deltas = {k: statuses[-1][k] - statuses[0][k] if all(k in x for x in statuses) else None
              for k in counter_fields}
    for k in counter_fields:
        if deltas[k] is not None:
            assert all(x[k] <= y[k] for x, y in zip(statuses, statuses[1:])), f"Counter regressed: {k}"
    writer_fields = ["queueFullSnapshotDrops", "disconnectedSnapshotDrops", "fileWriteErrors",
                     "serializationErrors", "oversizeSnapshotDrops", "workerSpawnErrors", "lastErrorCode"]
    async_writer = None
    if all("statusWriter" in x for x in statuses):
        async_writer = {"maxCounters": {k: max(x["statusWriter"][k] for x in statuses) for k in writer_fields},
                        "startCounters": {k: statuses[0]["statusWriter"][k] for k in writer_fields},
                        "endCounters": {k: statuses[-1]["statusWriter"][k] for k in writer_fields},
                        "deltaCounters": {k: statuses[-1]["statusWriter"][k] - statuses[0]["statusWriter"][k]
                                          for k in writer_fields},
                        "counterScope": "Cumulative process counters: a nonzero maximum may predate this trial; start/end/delta are separate. lastErrorCode is a status code, not an additive event count.",
                        "gpuSnapshotGenerationMsMax": max(x["statusSnapshotGenerationNs"] for x in statuses) / 1e6}
    return {
        "schemaVersion": 1,
        "scope": "Complete short quality-comparison trial. Source+AI excludes labeled repeats; no minimum-1s criterion or final180s acceptance is applied.",
        "samples": expected_samples, "pid": pid, "epoch": epoch, "surface": surface,
        "invariants": {"errorNullAllSamples": all(x["error"] is None for x in statuses),
                       "reportedErrors": [x["error"] for x in statuses if x["error"] is not None],
                       "worldObservedAllSamples": all(x["worldPlayerTaskObserved"] for x in statuses),
                       "actualGuestFinalSource": sorted({(x["guestFinalSource"]["width"], x["guestFinalSource"]["height"], x["guestFinalSource"]["actualFormat"]) for x in statuses}),
                       "actualOutputDimensions": sorted({(x["nativeBufferWidth"], x["nativeBufferHeight"]) for x in statuses}),
                       "physicalWindow": sorted({tuple(x["physicalWindow"]) for x in statuses}),
                       "configuredOutputFpsValues": sorted({x["configuredOutputFps"] for x in statuses})},
        "dedup": {"statusInput": sum(len(x["recentPresents"]) for x in statuses), "statusUnique": len(rows),
                  "statusConflicts": 0, "rsInput": raw, "rsUnique": len(pairs), "rsZero": zeros,
                  "rsInvalid": invalid, "otherOrUnknownSurfacePairsExcluded": other},
        "strict": {"method": "presentBegin<=RSflush<=presentComplete, bidirectionally unique; same-flush multiple-fence ambiguity excluded; no offsets or nearest matching",
                   "matchedAll": len(matched), "statusUnmatched": len(rows) - len(matched),
                   "rowMultiplePairs": sum(n > 1 for n in degree.values()),
                   "sameFlushMultiplePairs": sum(n > 1 for n in flush_degree.values()),
                   "windowRsPairsInclusive": window_pairs, "windowMatchedInclusive": window_matched,
                   "windowCoverageComplete": window_pairs == window_matched},
        "window": {"startFenceExclusive": start, "endFenceInclusive": end, "seconds": seconds,
                   "counts": counts, "sourcePlusAiCount": counts["source"] + counts["ai"],
                   "sourcePlusAiMeanHzExcludingRepeat": (counts["source"] + counts["ai"]) / seconds,
                   "sourceMeanHz": counts["source"] / seconds, "aiMeanHz": counts["ai"] / seconds,
                   "repeatMeanHz": counts["repeat"] / seconds, "allPresentMeanHz": len(arrivals) / seconds,
                   "meanAtLeast30Observed": (counts["source"] + counts["ai"]) / seconds >= 30,
                   "scope": "Fixed(first sample latest matched fence,last sample latest matched fence]; initial retained lookback excluded. Mean is not every-window stability or perceptual quality."},
        "counterDelta": deltas, "asyncWriter": async_writer,
        "inputProvenance": {"collection": provenance(collection_path),
                            "status": [provenance(directory / f"status-{i:02}.json") for i in range(expected_samples)],
                            "rs": [provenance(directory / f"rs-{i:02}.txt") for i in range(expected_samples)],
                            "analysisTool": provenance(Path(__file__))},
        "limitations": ["No missing events are filled or target-FPS-derived counts substituted.",
                        "Only this same-PID observed fence window is measured; changed settings need post-ApplySettings evidence.",
                        "Source/AI labels do not mean unique pixel hashes, photon timing or interpolation ground-truth accuracy.",
                        "Shader compilation/skipped-draw/host validation errors require the separate runtime log; a null FG error is not proof those counters are zero."]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--directory", required=True, type=Path)
    parser.add_argument("--expected-samples", required=True, type=int)
    parser.add_argument("--expected-pid", type=int)
    parser.add_argument("--surface", default="muguet-game-surfaceSurface")
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    directory, out = args.directory.resolve(), args.out.resolve()
    assert out != Path(__file__).resolve() and directory not in out.parents and out != directory, "Write outside raw input directory"
    result = analyze(directory, args.expected_samples, args.expected_pid, args.surface)
    out.write_text(json.dumps(result, indent=2, ensure_ascii=False, allow_nan=False) + "\n")
    print(json.dumps({"out": str(out), "pid": result["pid"], "samples": result["samples"],
                      "strict": result["strict"], "window": result["window"],
                      "invariants": result["invariants"]}, ensure_ascii=False))


if __name__ == "__main__":
    main()
