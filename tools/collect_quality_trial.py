#!/usr/bin/env python3
"""Collect a fixed 2s FG/RenderService trial using the current game's UI bounds.

The game must already be running. No application configuration is changed.
The collector fails closed on a different PID/epoch, stale status, a changed
surface, ambiguous W control or unsafe input coordinates. Command acknowledgments
are evidence of transport only, not proof that the game consumed an input.
Use --self-test for isolated fake-HDC tests; it never invokes a device command.
"""

import argparse
from collections import Counter
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

PERIOD = 2.0
BUNDLE = "com.minami.harmonyruntimeprobe"
SURFACE = "muguet-game-surfaceSurface"
STATUS = "/data/storage/el2/base/haps/entry/files/frame-interpolation-status.json"
DEFAULT_HDC = "/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/toolchains/hdc"
BOUNDS = re.compile(r"\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]")
CONTROL_LABELS = {"W↑", "A←", "S↓", "D→", "Shift", "Esc", "确认", "上车F", "跳跃",
                  "Q", "E", "R", "瞄准", "射击", "5", "返回", "Tab", "空格", "前标签", "后标签",
                  "上一页", "下一页", "↑", "←", "↓", "→"}


def utc():
    return datetime.now(timezone.utc).isoformat()


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, allow_nan=False) + "\n")


def load_json(path):
    if path.stat().st_size > 16 * 1024 * 1024:
        raise ValueError(f"JSON exceeds 16MiB: {path}")
    return json.loads(path.read_text(), parse_constant=lambda text: (_ for _ in ()).throw(
        ValueError(f"non-finite JSON constant: {text}")))


def positive_integer(value):
    return type(value) is int and value > 0


def physical_size(status):
    size = status.get("physicalWindow")
    if not isinstance(size, list) or len(size) != 2 or not all(positive_integer(v) for v in size):
        raise ValueError("status requires a positive integer physicalWindow pair")
    return tuple(size)


def validate_status(status, pid, epoch=None, physical=None, previous_elapsed=None):
    if not isinstance(status, dict) or status.get("pid") != pid or not positive_integer(status.get("pid")):
        raise ValueError(f"expected fresh PID {pid}, got {status.get('pid') if isinstance(status, dict) else None}")
    if not positive_integer(status.get("epoch")) or (epoch is not None and status["epoch"] != epoch):
        raise ValueError(f"epoch changed or invalid: {status.get('epoch')}, expected {epoch}")
    if not str(status.get("timestampClock", "")).startswith("CLOCK_MONOTONIC_ns_explicit_clock_gettime"):
        raise ValueError("explicit CLOCK_MONOTONIC status timestamps required")
    elapsed = status.get("elapsedSeconds")
    if isinstance(elapsed, bool) or not isinstance(elapsed, (int, float)) or not math.isfinite(elapsed) or elapsed < 0:
        raise ValueError("invalid elapsedSeconds")
    if previous_elapsed is not None and elapsed <= previous_elapsed:
        raise ValueError("status did not advance; refusing a stale process snapshot")
    actual = physical_size(status)
    if physical is not None and actual != physical:
        raise ValueError(f"physicalWindow changed from {physical} to {actual}; input layout invalidated")
    if not isinstance(status.get("recentPresents"), list) or not status["recentPresents"]:
        raise ValueError("FG status has no recent presents; wait for actual game rendering")
    return status


def bounds(value):
    match = BOUNDS.fullmatch(value) if isinstance(value, str) else None
    if not match:
        raise ValueError(f"invalid UI bounds: {value!r}")
    box = tuple(map(int, match.groups()))
    if box[0] >= box[2] or box[1] >= box[3]:
        raise ValueError(f"empty UI bounds: {box}")
    return box


def contains(outer, inner):
    return outer[0] <= inner[0] < inner[2] <= outer[2] and outer[1] <= inner[1] < inner[3] <= outer[3]


def point_inside(box, point):
    return box[0] <= point[0] < box[2] and box[1] <= point[1] < box[3]


def ui_nodes(tree):
    if isinstance(tree, dict):
        attributes = tree.get("attributes")
        if isinstance(attributes, dict):
            yield attributes
        for key, value in tree.items():
            if key != "attributes":
                yield from ui_nodes(value)
    elif isinstance(tree, list):
        for value in tree:
            yield from ui_nodes(value)


def visible(node):
    return node.get("visible") not in (False, "false") and node.get("enabled") not in (False, "false")


def normalize_label(text):
    return "".join(str(text).split())


def input_layout(tree, physical):
    nodes = list(ui_nodes(tree))
    root = bounds(tree.get("attributes", {}).get("bounds"))
    surfaces = {bounds(n["bounds"]) for n in nodes if visible(n) and n.get("type") == "XComponent"
                and "bounds" in n}
    surfaces = {box for box in surfaces if contains(root, box) and
                (box[2] - box[0], box[3] - box[1]) == physical}
    if len(surfaces) != 1:
        raise ValueError(f"require one XComponent matching physicalWindow {physical}, found {len(surfaces)}")
    surface = next(iter(surfaces))
    w_boxes = {bounds(n["bounds"]) for n in nodes if visible(n) and "bounds" in n and
               normalize_label(n.get("text", n.get("originalText", ""))) == "W↑"}
    if len(w_boxes) != 1:
        raise ValueError(f"require one visible enabled W↑ control, found {len(w_boxes)}; show operation controls")
    w_box = next(iter(w_boxes))
    if not contains(surface, w_box):
        raise ValueError("W↑ control extends outside the actual game surface")
    w_point = ((w_box[0] + w_box[2]) // 2, (w_box[1] + w_box[3]) // 2)
    blockers = set()
    for node in nodes:
        if not visible(node) or "bounds" not in node:
            continue
        label = normalize_label(node.get("text", node.get("originalText", "")))
        if node.get("type") in ("Button", "Toggle", "Slider", "Checkbox", "CheckBox") or label in CONTROL_LABELS:
            box = bounds(node["bounds"])
            if contains(root, box):
                blockers.add(box)
    # Candidate endpoints are proportions of the actual XComponent, including
    # its screen offset. A conservative bounding-box test avoids every button
    # rectangle with a small margin, not only the endpoint buttons.
    width, height = physical
    margin = max(8, round(min(physical) * 0.01))
    camera = None
    for y in (0.50, 0.40, 0.60, 0.30, 0.70):
        for left, right in ((0.48, 0.68), (0.40, 0.60), (0.30, 0.50)):
            first = (surface[0] + round(width * left), surface[1] + round(height * y))
            second = (surface[0] + round(width * right), surface[1] + round(height * (y + .025)))
            travel = (min(first[0], second[0]), min(first[1], second[1]),
                      max(first[0], second[0]) + 1, max(first[1], second[1]) + 1)
            collision = any(travel[0] < b[2] + margin and travel[2] > b[0] - margin and
                            travel[1] < b[3] + margin and travel[3] > b[1] - margin for b in blockers)
            if point_inside(surface, first) and point_inside(surface, second) and not collision:
                camera = (first, second)
                break
        if camera:
            break
    if camera is None:
        raise ValueError("no safe proportional camera path outside current button bounds")
    return {"physicalWindow": list(physical), "layoutRootBounds": list(root),
            "surfaceBounds": list(surface), "forwardLabel": "W ↑", "forwardBounds": list(w_box),
            "forwardPoint": list(w_point), "cameraStart": list(camera[0]), "cameraEnd": list(camera[1]),
            "cameraDurationMs": 600, "buttonAvoidanceMarginPx": margin,
            "buttonBoundsExcluded": [list(box) for box in sorted(blockers)],
            "coordinateScope": "absolute current UI coordinates; proportional camera path relative to actual XComponent; no rescaling of a previous device layout"}


class Device:
    def __init__(self, executable, target, runner=subprocess.run):
        self.prefix = [str(executable), "-t", target]
        self.runner = runner

    def call(self, arguments, timeout=20):
        command = self.prefix + arguments
        begin = time.monotonic()
        result = self.runner(command, capture_output=True, text=True, timeout=timeout)
        # Preserve the complete RS dump, including its leading surface header.
        # Truncating a command result before saving rs-NN.txt can silently lose
        # strict-match evidence. Small ACK previews are made only in summaries.
        return {"command": command, "exitCode": result.returncode, "stdout": result.stdout,
                "stderr": result.stderr, "wallSeconds": time.monotonic() - begin,
                "transportSucceeded": result.returncode == 0}

    @staticmethod
    def require(ack):
        if not ack["transportSucceeded"]:
            raise RuntimeError(f"HDC command failed: {ack}")

    def receive_status(self, bundle, destination):
        ack = self.call(["file", "recv", "-b", bundle, STATUS, str(destination)])
        self.require(ack)
        if not destination.is_file():
            raise RuntimeError("HDC reported success without producing a fresh status file")
        return load_json(destination), ack


def collect(args, runner=subprocess.run, clock=time.monotonic, sleeper=time.sleep):
    directory = args.directory.resolve()
    directory.mkdir(parents=True, exist_ok=True)
    reserved = [p for p in directory.iterdir() if p.name in
                ("collection.json", "inputs.json", "protocol.json", "preflight-status.json", "ui-layout.json", "collection-error.json")
                or re.fullmatch(r"(?:status-\d+\.json|rs-\d+\.txt)", p.name)]
    if reserved:
        raise ValueError("use a new trial directory; existing collector evidence must not be overwritten")
    device = Device(args.hdc, args.device, runner)
    summary = []
    inputs = {"schemaVersion": 1, "layout": None, "events": [],
              "ackScope": "HDC/uitest command acknowledgment only; game consumption and visual response require separate evidence"}
    try:
        initial, initial_ack = device.receive_status(args.bundle, directory / "preflight-status.json")
        validate_status(initial, args.expected_pid)
        epoch, physical = initial["epoch"], physical_size(initial)
        remote = f"/data/local/tmp/muguet-quality-layout-{uuid.uuid4().hex}.json"
        dump_ack = device.call(["shell", "uitest", "dumpLayout", "-p", remote])
        device.require(dump_ack)
        try:
            receive_ack = device.call(["file", "recv", remote, str(directory / "ui-layout.json")])
            device.require(receive_ack)
        finally:
            cleanup_ack = device.call(["shell", "rm", "-f", remote])
        layout = input_layout(load_json(directory / "ui-layout.json"), physical)
        inputs["layout"] = {**layout, "pid": args.expected_pid, "epoch": epoch,
                            "dumpAck": dump_ack, "receiveAck": receive_ack, "remoteCleanupAck": cleanup_ack,
                            "layoutSha256": hashlib.sha256((directory / "ui-layout.json").read_bytes()).hexdigest()}
        write_json(directory / "inputs.json", inputs)
        write_json(directory / "protocol.json", {"schemaVersion": 1, "device": args.device, "bundle": args.bundle,
                   "surface": args.surface, "expectedPid": args.expected_pid, "epoch": epoch,
                   "samples": args.samples, "intervalSeconds": PERIOD, "nominalSpanSeconds": (args.samples - 1) * PERIOD,
                   "preflightStatusAck": initial_ack, "layout": layout,
                   "movementSchedule": "W after i%10=5; right camera i%20=10; left camera i%20=0 and i>0; no input after final sample",
                   "rateScope": "No FPS is estimated by this collector; analyze fixed RS fence window with analyze_quality_trial.py; repeated presents are separate",
                   "collectorSha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()})
        start = clock()
        previous_elapsed = None
        for index in range(args.samples):
            deadline = start + index * PERIOD
            sleeper(max(0.0, deadline - clock()))
            began, began_utc = clock(), utc()
            status, transfer = device.receive_status(args.bundle, directory / f"status-{index:02}.json")
            query = device.call(["shell", "hidumper", "-s", "10", "-a", args.surface + " fps"])
            (directory / f"rs-{index:02}.txt").write_text(query["stdout"])
            device.require(query)
            validate_status(status, args.expected_pid, epoch, physical, previous_elapsed)
            previous_elapsed = status["elapsedSeconds"]
            row = {"sample": index, "utc": utc(), "hostMonotonic": clock(), "scheduledHostMonotonic": deadline,
                   "sampleBeginUtc": began_utc, "sampleBeginHostMonotonic": began,
                   "scheduleLatenessSeconds": max(0.0, began - deadline), "pid": status["pid"], "epoch": status["epoch"],
                   "elapsedSeconds": status["elapsedSeconds"], "physicalWindow": list(physical),
                   "output": [status["nativeBufferWidth"], status["nativeBufferHeight"]],
                   "sr": status["spatialUpscaling"], "world": status["worldPlayerTaskObserved"],
                   "sourcePresents": status["sourcePresents"], "aiPresents": status["aiPresents"],
                   "repeats": status["repeatPresents"], "missedSlots": status["missedDeadlineSlots"],
                   "recentKinds": dict(Counter(r["kind"] for r in status["recentPresents"])),
                   "error": status["error"], "statusAck": transfer,
                   "rsAck": {**query, "stdout": query["stdout"][:1024],
                             "stdoutBytes": len(query["stdout"].encode()), "stdoutScope": "preview; complete raw stdout is rs-NN.txt"}}
            scene_interval = getattr(args, "screenshot_every", 0)
            if scene_interval and (index % scene_interval == 0 or index + 1 == args.samples):
                image_path = directory / f"scene-{index:02}.jpeg"
                image_remote = f"/data/local/tmp/muguet-quality-scene-{uuid.uuid4().hex}.jpeg"
                image_capture = device.call(["shell", "snapshot_display", "-f", image_remote])
                device.require(image_capture)
                try:
                    image_receive = device.call(["file", "recv", image_remote, str(image_path)])
                    device.require(image_receive)
                    if not image_path.is_file():
                        raise RuntimeError("scene image was not received")
                finally:
                    image_cleanup = device.call(["shell", "rm", "-f", image_remote])
                row["sceneImage"] = {"path": image_path.name, "sha256": hashlib.sha256(image_path.read_bytes()).hexdigest(),
                                     "captureAck": image_capture, "receiveAck": image_receive,
                                     "cleanupAck": image_cleanup,
                                     "scope": "Actual screen near this sample; requires visual review for gameplay versus a modal or pause screen"}
            summary.append(row)
            write_json(directory / "collection.json", summary)
            print(json.dumps({k: row[k] for k in ("sample", "pid", "epoch", "elapsedSeconds", "output", "world", "sourcePresents", "aiPresents", "repeats", "error")}), flush=True)
            command, description = None, None
            if index + 1 < args.samples:
                if index % 10 == 5:
                    command = ["longClick", *map(str, layout["forwardPoint"])]
                    description = "hold current W↑ control; uitest default longClick duration"
                elif index > 0 and index % 10 == 0:
                    first, second = layout["cameraStart"], layout["cameraEnd"]
                    if index % 20 == 0:
                        first, second = second, first
                    command = ["swipe", *map(str, first), *map(str, second), str(layout["cameraDurationMs"])]
                    description = "camera swipe avoiding current UI buttons"
            if command:
                event = {"afterSample": index, "pid": args.expected_pid, "epoch": epoch, "description": description,
                         "inputCommand": command, "startUtc": utc(), "startHostMonotonic": clock()}
                inputs["events"].append(event)
                write_json(directory / "inputs.json", inputs)
                try:
                    ack = device.call(["shell", "uitest", "uiInput"] + command, timeout=10)
                    event.update(ack)
                    event["ackTextHasError"] = bool(re.search(r"(?im)^\s*(?:error\b|failed\b|\[error\])", ack["stdout"] + "\n" + ack["stderr"]))
                except Exception as error:
                    event["exception"] = str(error)
                    raise
                finally:
                    event.update(endUtc=utc(), endHostMonotonic=clock())
                    write_json(directory / "inputs.json", inputs)
                device.require(ack)
                if event["ackTextHasError"]:
                    raise RuntimeError("uitest returned an error acknowledgment; input trial is incomplete")
        print("collection complete", flush=True)
        return summary
    except Exception as error:
        write_json(directory / "collection-error.json", {"error": str(error), "utc": utc(),
                   "completedSamples": len(summary), "expectedSamples": args.samples,
                   "expectedPid": args.expected_pid, "device": args.device})
        raise


def self_test():
    """Actual collector with a fake transport and clock, plus strict analyzer."""
    import contextlib
    import io
    from types import SimpleNamespace
    passed = []

    def check(name, operation):
        operation()
        passed.append(name)

    def fixture(width=2442, height=1140, top=0):
        surface = (0, top, width, top + height)
        w = (round(width * .06), top + round(height * .80), round(width * .12), top + round(height * .87))
        node = lambda kind, box, text="": {"attributes": {"type": kind, "bounds": f"[{box[0]},{box[1]}][{box[2]},{box[3]}]", "text": text, "visible": "true", "enabled": "true"}, "children": []}
        tree = node("", (0, 0, width, top + height))
        tree["children"] = [node("XComponent", surface), node("Text", w, "W ↑"), node("Button", (width - 200, top + 10, width - 10, top + 90), "首页")]
        return tree

    for width, height, top in [(2442, 1140, 0), (3000, 2000, 0), (3184, 2115, 117), (1140, 2442, 0)]:
        def geometry(w=width, h=height, t=top):
            layout = input_layout(fixture(w, h, t), (w, h))
            assert point_inside(layout["forwardBounds"], layout["forwardPoint"])
            assert point_inside(layout["surfaceBounds"], layout["cameraStart"])
            assert point_inside(layout["surfaceBounds"], layout["cameraEnd"])
            if (w, h) == (2442, 1140):
                assert layout["forwardPoint"][1] < 1140
                assert layout["forwardPoint"] != [242, 1666]
        check(f"current bounds/proportions {width}x{height} top={top}", geometry)

    def rejected(operation):
        try:
            operation()
        except (ValueError, RuntimeError):
            return
        raise AssertionError("expected fail-closed rejection")

    tree = fixture(); tree["children"] = tree["children"][:1]
    check("missing W control rejects", lambda: rejected(lambda: input_layout(tree, (2442, 1140))))
    tree = fixture(); tree["children"].append({"attributes": {"type": "Text", "text": "W↑", "bounds": "[600,700][700,800]"}})
    check("ambiguous W control rejects", lambda: rejected(lambda: input_layout(tree, (2442, 1140))))
    check("physical/layout mismatch rejects", lambda: rejected(lambda: input_layout(fixture(), (3000, 2000))))
    blocked = fixture(); blocked["children"].append({"attributes": {"type": "Button", "bounds": "[900,500][1900,650]"}})
    check("camera avoids a center overlay button", lambda: assert_camera_avoids(blocked))

    class FakeClock:
        value = 1000.0
        def now(self): return self.value
        def sleep(self, delay): self.value += delay

    class FakeHdc:
        def __init__(self, bad=None):
            self.calls, self.receives, self.rows, self.bad = [], 0, [], bad
        def run(self, command, **kwargs):
            self.calls.append(command)
            assert command[:3] == ["FAKE-HDC-NOT-EXECUTED", "-t", "mock-device"]
            args = command[3:]
            text, code = "ok\n", 0
            if args[:3] == ["file", "recv", "-b"]:
                self.receives += 1
                count = self.receives * 60
                rows = [{"slot": i, "kind": "source" if i % 2 == 0 else "ai", "nativePresentCalled": True,
                         "presentBeginMonotonicNs": 10**12 + i * 33333333,
                         "presentCompleteMonotonicNs": 10**12 + i * 33333333 + 200000} for i in range(max(1, count - 179), count + 1)]
                self.rows = rows
                status = {"pid": 1234, "epoch": 1, "physicalWindow": [2442, 1140], "timestampClock": "CLOCK_MONOTONIC_ns_explicit_clock_gettime",
                          "elapsedSeconds": self.receives * 2, "recentPresents": rows, "nativeBufferWidth": 2314, "nativeBufferHeight": 1080,
                          "spatialUpscaling": True, "worldPlayerTaskObserved": True, "sourcePresents": count // 2,
                          "aiPresents": count // 2, "repeatPresents": 0, "missedDeadlineSlots": 0, "error": None,
                          "guestFinalSource": {"width": 1714, "height": 800, "actualFormat": "Bgra8Unorm"}, "configuredOutputFps": 48}
                if self.bad == "pid" or self.bad == "later-pid" and self.receives == 3: status["pid"] = 999
                if self.bad == "epoch" and self.receives == 3: status["epoch"] = 2
                if self.bad == "resize" and self.receives == 3: status["physicalWindow"] = [3000, 2000]
                if self.bad == "stale" and self.receives == 3: status["elapsedSeconds"] = 4
                write_json(Path(args[-1]), status)
            elif args[:2] == ["file", "recv"]:
                write_json(Path(args[-1]), fixture())
            elif args[:2] == ["shell", "hidumper"]:
                text = "surface [muguet-game-surfaceSurface]:\n" + "\n".join(
                    str(r["presentBeginMonotonicNs"] + 100000) + ":" + str(r["presentBeginMonotonicNs"] + 1000000) for r in self.rows) + "\n" * 20000
            elif args[:3] == ["shell", "uitest", "uiInput"] and self.bad == "input":
                text, code = "Error: injection rejected\n", 1
            return subprocess.CompletedProcess(command, code, text, "")

    with tempfile.TemporaryDirectory(prefix="quality-collector-contract-") as temporary:
        root = Path(temporary)
        def run_case(name, bad=None, samples=22):
            args = SimpleNamespace(directory=root / name, device="mock-device", hdc="FAKE-HDC-NOT-EXECUTED",
                                   bundle=BUNDLE, surface=SURFACE, expected_pid=1234, samples=samples)
            fake, clock = FakeHdc(bad), FakeClock()
            with contextlib.redirect_stdout(io.StringIO()):
                result = collect(args, fake.run, clock.now, clock.sleep)
            return args, fake, result
        args, fake, result = run_case("valid")
        check("fixed 2s schedule and explicit target on every fake call", lambda: require(
            len(result) == 22 and all(x["scheduledHostMonotonic"] == 1000 + i * 2 for i, x in enumerate(result))))
        inputs = load_json(args.directory / "inputs.json")
        check("current-layout W and bidirectional camera command ACKs recorded", lambda: require(
            len(inputs["events"]) == 4 and all(x["transportSucceeded"] for x in inputs["events"])
            and inputs["events"][0]["inputCommand"][0] == "longClick"
            and inputs["events"][1]["inputCommand"][1:3] == list(map(str, inputs["layout"]["cameraStart"]))
            and inputs["events"][3]["inputCommand"][1:3] == list(map(str, inputs["layout"]["cameraEnd"])) ))
        spec = importlib.util.spec_from_file_location("strict_quality_analysis", Path(__file__).with_name("analyze_quality_trial.py"))
        analyzer = importlib.util.module_from_spec(spec); spec.loader.exec_module(analyzer)
        analysis = analyzer.analyze(args.directory, 22, 1234)
        check("existing strict analyzer accepts complete collector layout with full RS coverage", lambda: require(
            analysis["samples"] == 22 and analysis["strict"]["windowCoverageComplete"] and analysis["window"]["counts"]["repeat"] == 0))
        check("refuses overwriting existing trial evidence", lambda: rejected(lambda: collect(args, fake.run)))
        for bad in ("pid", "later-pid", "epoch", "resize", "stale", "input"):
            check("rejects " + bad + " and keeps incomplete evidence", lambda b=bad: rejected(lambda: run_case(b, b)))
            error = load_json(root / bad / "collection-error.json")
            assert error["completedSamples"] < 22
            if bad == "pid":
                assert not (root / bad / "inputs.json").exists()
    print(json.dumps({"ok": True, "contracts": len(passed), "cases": passed,
                      "scope": "fake transport/clock and local temporary files only; no HDC/device operation"}, indent=2))


def require(value):
    if not value:
        raise AssertionError("contract failed")


def assert_camera_avoids(tree):
    layout = input_layout(tree, (2442, 1140))
    require(layout["cameraStart"][1] < 490 or layout["cameraStart"][1] > 660)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--device", help="Explicit HDC target; never uses an implicit default device")
    parser.add_argument("--directory", type=Path, help="New raw-evidence directory")
    parser.add_argument("--samples", type=int, help="2s samples: 31=60s, 46=90s, 91=180s")
    parser.add_argument("--expected-pid", type=int, help="Actual current game PID obtained independently of a possibly stale status file")
    parser.add_argument("--screenshot-every", type=int, default=0, help="Capture actual screen every N samples and at the final sample; 0 disables scene images")
    parser.add_argument("--hdc", default=DEFAULT_HDC if Path(DEFAULT_HDC).is_file() else shutil.which("hdc"))
    parser.add_argument("--bundle", default=BUNDLE)
    parser.add_argument("--surface", default=SURFACE)
    parser.add_argument("--self-test", action="store_true", help="Run fake-HDC host contracts; never invokes subprocess/device tools")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if not args.device or not args.directory or not args.expected_pid or not args.samples or not args.hdc:
        parser.error("--device, --directory, --samples, --expected-pid and an available --hdc are required")
    if not positive_integer(args.expected_pid) or not 2 <= args.samples <= 1000:
        parser.error("require positive expected PID and 2..1000 samples")
    try:
        collect(args)
    except (ValueError, RuntimeError, OSError, subprocess.SubprocessError) as error:
        print(f"collection failed: {error}", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
