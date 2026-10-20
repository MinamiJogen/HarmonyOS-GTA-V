#!/usr/bin/env python3
"""Synthetic provenance and strict-FPS regressions for actual source pairing."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

import analyze_game_fg as fg
import analyze_quality_trial as quality


def metadata(slot, kind, sequence, source_slot, **extra):
    return {"outputSlot": slot, "kind": kind, "sourceSequence": sequence,
            "sourceSlot": source_slot, **extra}


def item(row):
    return {"outputSlot": row["outputSlot"], "outputProvenance": row}


class EndpointTests(unittest.TestCase):
    def setUp(self):
        self.rows = [metadata(20, "source", 99, 10),
                     metadata(22, "ai", 99, 10, newerSourceSlot=12, newerSourceSequence=103),
                     metadata(24, "source", 103, 12)]

    def link(self, rows=None):
        return fg.capture_endpoint_link(*map(item, rows or self.rows))

    def test_even_midpoint_and_nonadjacent_real_endpoints(self):
        result = self.link()
        self.assertTrue(result["endpointProvenanceVerified"])
        self.assertFalse(result["legacyAdjacencyUsed"])
        self.assertFalse(result["outputSlotsConsecutive"])

    def test_mismatch_slot_sequence_old_or_malformed_explicit_never_falls_back(self):
        for index, key, value in [(1, "newerSourceSlot", 13), (1, "newerSourceSequence", 104),
                                  (1, "sourceSequence", 98), (1, "newerSourceSlot", None),
                                  (1, "newerSourceSequence", True), (2, "sourceSlot", True)]:
            with self.subTest(key=key, value=value):
                rows = copy.deepcopy(self.rows)
                rows[index][key] = value
                self.assertFalse(self.link(rows)["endpointProvenanceVerified"])

    def test_legacy_fallback_is_limited_to_adjacent_output_and_source_slots(self):
        rows = [metadata(20, "source", 99, 10), metadata(21, "ai", 99, 10),
                metadata(22, "source", 103, 11)]
        self.assertTrue(self.link(rows)["endpointProvenanceVerified"])
        self.assertTrue(self.link(rows)["legacyAdjacencyUsed"])
        rows[2]["sourceSlot"] = 12
        self.assertFalse(self.link(rows)["endpointProvenanceVerified"])
        rows[2]["sourceSlot"] = 11
        rows[2]["outputSlot"] = 24
        self.assertFalse(self.link(rows)["endpointProvenanceVerified"])

    def capture(self, root, rows, capture_pid=123):
        frames = []
        for i, row in enumerate(rows):
            filename = f"frame-{i}.ppm"
            # Deliberately distinct pixels; no frame is a claimed GT reference.
            (root / filename).write_bytes(b"P6\n2 1\n255\n" + bytes([i * 11 + j for j in range(6)]))
            frames.append({"captureSequence": i, "sourceSequence": row["outputSlot"],
                           "width": 2, "height": 1, "image": {"filename": filename}})
        (root / "capture-status.json").write_text(json.dumps({
            "pid": capture_pid, "sourceSequenceMeaning": "output_slot_index", "frames": frames}))
        return fg.capture_facts(root, {"pid": 123, "outputCaptureFrames": rows})

    def test_captured_intervening_repeats_do_not_break_actual_endpoint_join(self):
        rows = [self.rows[0], metadata(21, "repeat", 99, 10), self.rows[1],
                metadata(23, "repeat", 99, 10), self.rows[2]]
        with tempfile.TemporaryDirectory() as path:
            result = self.capture(Path(path), rows)
        self.assertEqual(len(result["sourceAiSourceTriplets"]), 1)
        self.assertEqual(result["consecutiveSourceAiSourceTriplets"], [])
        self.assertEqual(result["sourceAiSourceTriplets"][0]["outputSlots"], [20, 22, 24])
        self.assertIn("differentPixels", result["sourceAiSourceTriplets"][0])

    def test_missing_or_cross_pid_endpoints_cannot_be_attributed(self):
        with tempfile.TemporaryDirectory() as path:
            root = Path(path)
            missing = self.capture(root, self.rows[1:])
            self.assertEqual(missing["sourceAiSourceTriplets"], [])
            self.assertEqual(missing["sourceAiEndpointLinks"][0]["verifiedEndpointPairCandidates"], 0)
            cross_pid = self.capture(root, self.rows, capture_pid=456)
            self.assertFalse(cross_pid["outputSlotMappingEnabled"])
            self.assertEqual(cross_pid["sourceAiSourceTriplets"], [])

    def test_duplicate_capture_slot_blocks_endpoint_mapping(self):
        with tempfile.TemporaryDirectory() as path:
            result = self.capture(Path(path), [self.rows[0], *self.rows])
        self.assertEqual(result["sourceAiSourceTriplets"], [])


class StrictRateTests(unittest.TestCase):
    def setUp(self):
        self.rows = []
        self.rs = []
        for i, (slot, kind) in enumerate([(20, "source"), (21, "repeat"), (22, "ai"),
                                         (23, "repeat"), (24, "source")]):
            begin = 1_000_000_000 + i * 100_000_000
            self.rows.append({"slot": slot, "kind": kind, "nativePresentCalled": True,
                              "presentBeginMonotonicNs": begin, "presentCompleteMonotonicNs": begin + 2,
                              "sourceSlot": 12 if i == 4 else 10,
                              "sourceSequence": 103 if i == 4 else 99,
                              "newerSourceSlot": 12 if kind == "ai" else None,
                              "newerSourceSequence": 103 if kind == "ai" else None})
            self.rs.append({"flushNs": begin + 1, "presentFenceNs": begin + 3})

    def test_game_analysis_strict_matching_keeps_even_ai_and_provenance(self):
        result = fg.match_surface({"surfaceName": "game", "records": self.rs,
                                   "duplicateExactPairsExcluded": 0}, self.rows, True)
        matched = result["uniqueMatchedWindow"]
        self.assertEqual(result["matching"]["matchedRecords"], 5)
        self.assertEqual(matched["kindCounts"], {"source": 2, "ai": 1, "repeat": 2})
        self.assertEqual(matched["records"][2]["newerSourceSlot"], 12)

    def test_quality_trial_strict_rate_has_no_slot_parity_or_pair_adjacency_filter(self):
        with tempfile.TemporaryDirectory() as path:
            root = Path(path)
            (root / "collection.json").write_text(json.dumps([{"sample": 0}, {"sample": 1}]))
            for i, subset in enumerate((self.rows[:2], self.rows[2:])):
                status = {"pid": 123, "epoch": 1, "elapsedSeconds": i + 1,
                          "timestampClock": "CLOCK_MONOTONIC_ns_explicit_clock_gettime",
                          "recentPresents": subset, "error": None, "worldPlayerTaskObserved": True,
                          "guestFinalSource": {"width": 1714, "height": 800, "actualFormat": "BGRA8_UNORM"},
                          "nativeBufferWidth": 2314, "nativeBufferHeight": 1080,
                          "physicalWindow": [2442, 1140], "configuredOutputFps": 48}
                (root / f"status-{i:02}.json").write_text(json.dumps(status))
                (root / f"rs-{i:02}.txt").write_text("surface [muguet-game-surfaceSurface]:\n" +
                    "".join(f"{row['flushNs']}:{row['presentFenceNs']}\n" for row in self.rs))
            result = quality.analyze(root, 2, 123)
        self.assertEqual(result["strict"]["matchedAll"], 5)
        self.assertTrue(result["strict"]["windowCoverageComplete"])
        self.assertEqual(result["window"]["counts"], {"source": 1, "ai": 1, "repeat": 1})
        self.assertEqual(result["window"]["sourcePlusAiCount"], 2)
        self.assertAlmostEqual(result["window"]["sourcePlusAiMeanHzExcludingRepeat"], 2 / .3)


if __name__ == "__main__":
    unittest.main()
