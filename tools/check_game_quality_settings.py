#!/usr/bin/env python3
"""Check original post-ApplySettings fields, argv and launcher preset agreement.

Example:
  python3 tools/check_game_quality_settings.py --expected high \
    --log actual-settings-log.txt --out settings-check.json

Use --presets and --game-quality with the matching report/source snapshot when
checking a later tuned high candidate or a historical binary. This checks
settings evidence, not rendering quality, FPS, binary identity or shader efficacy.
For a tuned candidate, --override ReflectionQuality=1 updates the expected field
and its associated original CLI argument without rewriting the frozen report.
Only the requested output file is written. No game code is executed.
"""

import argparse
from decimal import Decimal, InvalidOperation
import hashlib
import json
import math
from pathlib import Path
import re
import sys


ROOT = Path(__file__).resolve().parents[1]
DEFAULT_PRESETS = ROOT / "evidence/research/original-game-quality-presets-20261020.json"
DEFAULT_SOURCE = ROOT / "harmony/entry/src/main/ets/config/GameQuality.ets"
FLOAT_TOLERANCE = 0.00001
# Original argument-to-field relationships. Values live in the supplied report,
# not this map, so tuning a candidate does not require changing this program.
FIELD_ARGS = {
    "TextureQuality": "textureQuality", "ShaderQuality": "shaderQuality",
    "ShadowQuality": "shadowQuality", "Shadow_SoftShadows": "shadowSoftness",
    "ReflectionQuality": "reflectionQuality", "ReflectionMSAA": "reflectionMSAA",
    "SSAO": "SSAO", "AnisotropicFiltering": "anisotropicQualityLevel",
    "MSAA": "multiSample", "MSAAQuality": "multiSampleQuality",
    "SamplingMode": "samplingMode", "ParticleQuality": "particleQuality",
    "WaterQuality": "waterQuality", "GrassQuality": "grassQuality",
    "PostFX": "postFX", "LodScale": "lodScale", "PedLodBias": "pedLodBias",
    "VehicleLodBias": "vehicleLodBias", "CityDensity": "cityDensity",
    "PedVarietyMultiplier": "pedVariety", "VehicleVarietyMultiplier": "vehicleVariety",
    "FXAA_Enabled": "fxaa", "TXAA_Enabled": "txaa",
    "Reflection_MipBlur": "reflectionBlur", "Tessellation": "tessellation",
    "MaxLodScale": "MaxLODScale", "MotionBlurStrength": "MotionBlurStrength",
    "UltraShadows_Enabled": "ultraShadows", "Shadow_ParticleShadows": "particleShadows",
    "Shadow_LongShadows": "shadowLongShadows", "Shadow_Distance": "shadowDistance",
    "Shadow_DisableScreenSizeCheck": "shadowDisableScreenSizeCheck",
    "Lighting_FogVolumes": "fogVolumes", "HdStreamingInFlight": "HDStreamingInFlight",
    "DoF": "noInGameDOF", "Shadow_SplitZStart": "shadowSplitZStart",
    "Shadow_SplitZEnd": "shadowSplitZEnd", "Shadow_aircraftExpWeight": "shadowAircraftWeight",
}


def artifact(path, data=None):
    if data is None:
        data = path.read_bytes()
    return {"path": str(path.resolve()), "bytes": len(data),
            "sha256": hashlib.sha256(data).hexdigest()}


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"Duplicate JSON key: {key}")
        result[key] = value
    return result


def clean_line(line):
    # Supports both the exported [eng] excerpt and the same lines in runtime.log.
    return re.sub(r"^.*?\[eng(?::\d+)?\]\s*", "", line).strip()


def settings_blocks(text):
    blocks = []
    current = None
    in_argv = False
    for number, raw in enumerate(text.splitlines(), 1):
        line = clean_line(raw)
        if line == "[settings] Using Settings:":
            current = {"startLine": number, "fields": {}, "duplicateFields": [],
                       "argv": [], "argvComplete": False, "argvStartLine": None}
            blocks.append(current)
            in_argv = False
            continue
        if current is None:
            continue
        if line == "CommandLine":
            if current["argvStartLine"] is not None:
                # An unrelated subsequent argv cannot repair a missing first end.
                current["argvDuplicateStart"] = True
            current["argvStartLine"] = number
            in_argv = True
            continue
        if line == "/CommandLine" and in_argv:
            current["argvComplete"] = True
            current["argvEndLine"] = number
            in_argv = False
            continue
        if in_argv:
            if line.startswith("-"):
                current["argv"].append(line)
            continue
        # Only the settings section preceding this block's argv owns these fields.
        if current["argvStartLine"] is not None:
            continue
        match = re.fullmatch(r"\[settings\]\s+([A-Za-z][A-Za-z0-9_]*):\s*(.*?)", line)
        if match:
            key, value = match.groups()
            if key in current["fields"]:
                current["duplicateFields"].append(key)
            else:
                current["fields"][key] = {"raw": value, "line": number}
    return blocks


def finite_decimal(value):
    if not isinstance(value, str) or len(value) > 256:
        return None
    try:
        number = Decimal(value)
    except (InvalidOperation, TypeError):
        return None
    return number if number.is_finite() and (number.is_zero() or abs(number.adjusted()) <= 308) else None


def compare_settings(expected, observed):
    missing, mismatches, matched = [], [], []
    for key, wanted in expected.items():
        item = observed.get(key)
        if item is None:
            missing.append(key)
            continue
        raw = item["raw"]
        if type(wanted) is bool:
            actual = {"true": True, "false": False}.get(raw)
            equal = type(actual) is bool and actual == wanted
        elif type(wanted) in (int, float) and math.isfinite(wanted):
            number = finite_decimal(raw)
            actual = None if number is None else float(number)
            if actual is not None and not math.isfinite(actual):
                actual = None
            equal = number is not None and abs(number - Decimal(str(wanted))) <= Decimal(str(FLOAT_TOLERANCE))
        else:
            raise ValueError(f"Unsupported expected setting type/value: {key}={wanted!r}")
        if equal:
            matched.append(key)
        else:
            mismatches.append({"field": key, "expected": wanted, "actual": actual,
                               "actualRaw": raw, "line": item["line"]})
    return {"expectedCount": len(expected), "matchedCount": len(matched),
            "missing": missing, "mismatches": mismatches,
            "absoluteNumericTolerance": FLOAT_TOLERANCE}


def arg_map(args):
    result, duplicates = {}, []
    for arg in args:
        if not isinstance(arg, str) or not arg.startswith("-"):
            raise ValueError(f"Not an engine argument: {arg!r}")
        key, separator, value = arg.partition("=")
        key = key.lower()
        if key in result:
            duplicates.append(key)
        result[key] = value if separator else None
    return result, duplicates


def same_arg_value(left, right):
    if left is None or right is None:
        return left is right
    a, b = finite_decimal(left), finite_decimal(right)
    return a == b if a is not None and b is not None else left == right


def compare_arguments(expected, actual, allow_extra=False):
    wanted, expected_duplicates = arg_map(expected)
    observed, actual_duplicates = arg_map(actual)
    missing = sorted(set(wanted) - set(observed))
    extra = sorted(set(observed) - set(wanted))
    mismatches = [{"argument": key, "expected": value, "actual": observed[key]}
                  for key, value in wanted.items()
                  if key in observed and not same_arg_value(value, observed[key])]
    duplicates = sorted(set(actual_duplicates) & set(wanted))
    return {"expectedCount": len(wanted), "matchedCount": len(wanted) - len(missing) - len(mismatches),
            "missing": missing, "mismatches": mismatches, "duplicateManagedArguments": duplicates,
            "duplicateExpectedArguments": expected_duplicates,
            "unexpectedArguments": [] if allow_extra else extra,
            "unmanagedArguments": extra if allow_extra else []}


def strip_comments(text):
    # Keep literals intact; never regex-remove a // sequence inside a literal.
    result, position = [], 0
    while position < len(text):
        if text.startswith("//", position):
            end = text.find("\n", position)
            position = len(text) if end < 0 else end
        elif text.startswith("/*", position):
            end = text.find("*/", position + 2)
            if end < 0:
                raise ValueError("Unclosed source comment")
            result.append(" ")
            position = end + 2
        elif text[position] in "'\"`":
            end, _ = read_literal(text, position)
            result.append(text[position:end])
            position = end
        else:
            result.append(text[position])
            position += 1
    return "".join(result)


def read_literal(text, position):
    quote, start = text[position], position + 1
    position += 1
    while position < len(text):
        if text[position] == "\\":
            position += 2
        elif text[position] == quote:
            return position + 1, text[start:position]
        else:
            position += 1
    raise ValueError("Unclosed source literal")


def mask_literals(text):
    masked, position = list(text), 0
    while position < len(text):
        if text[position] in "'\"`":
            end, _ = read_literal(text, position)
            masked[position:end] = ["\n" if char == "\n" else " " for char in text[position:end]]
            position = end
        else:
            position += 1
    return "".join(masked)


class NumericConditional:
    """Restricted numeric/boolean ternaries; no JavaScript eval or imports."""
    TOKEN = re.compile(r"\s*(low|high|true|false|[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?|[?:()])")

    def __init__(self, expression, preset):
        self.tokens, position = [], 0
        while position < len(expression):
            match = self.TOKEN.match(expression, position)
            if not match:
                if expression[position:].strip():
                    raise ValueError(f"Unsupported preset expression: {expression!r}")
                break
            self.tokens.append(match[1])
            position = match.end()
        self.position, self.preset = 0, preset

    def primary(self):
        if self.position >= len(self.tokens):
            raise ValueError("Incomplete preset expression")
        token = self.tokens[self.position]
        self.position += 1
        if token == "(":
            value = self.conditional()
            self.take(")")
            return value
        if token in ("low", "high"):
            return self.preset == token
        if token in ("true", "false"):
            return token == "true"
        if finite_decimal(token) is None:
            raise ValueError(f"Invalid numeric preset token: {token}")
        return token

    def take(self, token):
        if self.position >= len(self.tokens) or self.tokens[self.position] != token:
            raise ValueError(f"Expected {token!r} in preset expression")
        self.position += 1

    def conditional(self):
        value = self.primary()
        if self.position < len(self.tokens) and self.tokens[self.position] == "?":
            self.position += 1
            if type(value) is not bool:
                raise ValueError("Preset conditional must test a boolean")
            yes = self.conditional()
            self.take(":")
            no = self.conditional()
            return yes if value else no
        return value

    def evaluate(self):
        value = self.conditional()
        if self.position != len(self.tokens) or type(value) is not str:
            raise ValueError("Preset interpolation must produce exactly one numeric value")
        return value


def source_arguments(source, preset):
    text = strip_comments(source)
    declaration = re.search(r"export\s+function\s+gameQualityArguments\s*\(\s*preset\s*:\s*string\s*\)\s*:\s*string\[\]\s*\{", mask_literals(text))
    if not declaration:
        raise ValueError("Cannot locate gameQualityArguments(preset: string): string[]")
    position, depth = declaration.end(), 1
    start = position
    while position < len(text) and depth:
        char = text[position]
        if char in "'\"`":
            position, _ = read_literal(text, position)
            continue
        depth += (char == "{") - (char == "}")
        position += 1
    if depth:
        raise ValueError("Unclosed gameQualityArguments function")
    body = text[start:position - 1]
    definitions = []
    for _ in range(2):
        definition = re.match(r"\s*const\s+(low|high)\s*:\s*boolean\s*=\s*preset\s*===\s*(['\"])(low|high)\2\s*;", body)
        if not definition:
            raise ValueError("Unsupported or changed preset boolean declarations")
        definitions.append((definition[1], definition[3]))
        body = body[definition.end():]
    if sorted(definitions) != [("high", "high"), ("low", "low")]:
        raise ValueError("Unsupported or changed preset boolean declarations")
    body = body.strip()
    if not body.startswith("return"):
        raise ValueError("Unsupported statements before preset argument array")
    body = body[len("return"):].strip()
    if not body.startswith("["):
        raise ValueError("Preset arguments must be a literal array")
    position, result, need_item = 1, [], True
    while position < len(body):
        if body[position].isspace():
            position += 1
            continue
        if body[position] == "]":
            position += 1
            if body[position:].strip() not in ("", ";"):
                raise ValueError("Unsupported statements after preset argument array")
            return result
        if not need_item or body[position] not in "'\"`":
            raise ValueError("Preset array supports only comma-separated string literals")
        quote = body[position]
        position, literal = read_literal(body, position)
        if "\\" in literal:
            raise ValueError("Escaped preset literals are not supported by the read-only parser")
        if quote == "`":
            literal = re.sub(r"\$\{([^{}]+)\}", lambda match: NumericConditional(match[1], preset).evaluate(), literal)
            if "${" in literal:
                raise ValueError("Unsupported nested template interpolation")
        result.append(literal)
        need_item = False
        while position < len(body) and body[position].isspace():
            position += 1
        if position < len(body) and body[position] == ",":
            position += 1
            need_item = True
    raise ValueError("Unclosed preset argument array")


def apply_overrides(expected_fields, expected_args, overrides):
    fields, args, changes = dict(expected_fields), list(expected_args), []
    seen = set()
    for override in overrides:
        field, separator, raw = override.partition("=")
        if not separator or field not in fields or field not in FIELD_ARGS or field in seen:
            raise ValueError(f"Unknown, duplicate or independently unrepresentable field override: {override}")
        value = json.loads(raw)
        previous = fields[field]
        if type(previous) is bool:
            valid = type(value) is bool
        else:
            valid = type(value) in (int, float) and math.isfinite(value)
        if not valid:
            raise ValueError(f"Override type must match the expected setting: {override}")
        cli_value = value
        if field == "ShadowQuality":
            if type(value) is not int or not 0 <= value <= 3:
                raise ValueError("ShadowQuality override is an internal integer level 0..3")
            cli_value = value - 1
        if type(cli_value) is bool:
            cli_value = int(cli_value)
        key = "-" + FIELD_ARGS[field].lower()
        matches = [i for i, argument in enumerate(args) if argument.partition("=")[0].lower() == key]
        if len(matches) != 1:
            raise ValueError(f"Override needs exactly one associated expected CLI: {field}")
        index = matches[0]
        previous_arg = args[index]
        args[index] = previous_arg.partition("=")[0] + "=" + str(cli_value)
        fields[field] = value
        change = {"field": field, "originalExpected": previous, "overrideExpected": value,
                  "originalArgument": previous_arg, "overrideArgument": args[index]}
        if field == "ShaderQuality" and "Shader_SSA" in fields:
            fields["Shader_SSA"] = value > 0
            change["derivedShader_SSA"] = fields["Shader_SSA"]
        changes.append(change)
        seen.add(field)
    return fields, args, changes


def check(log, presets, source, preset, block_index=None, overrides=()):
    # Hash exactly the byte snapshots that were parsed, even if a caller supplies
    # a live append-only runtime.log instead of a frozen excerpt.
    log_bytes, presets_bytes, source_bytes = log.read_bytes(), presets.read_bytes(), source.read_bytes()
    expected_report = json.loads(presets_bytes, object_pairs_hook=unique_object)
    candidate = expected_report["presets"][preset]
    expected_fields, expected_args = candidate["expectedPostApplySettings"], candidate["engineArgs"]
    if not isinstance(expected_fields, dict) or not expected_fields or not isinstance(expected_args, list) or not expected_args:
        raise ValueError("Expected preset must contain nonempty field and argument collections")
    expected_fields, expected_args, changes = apply_overrides(expected_fields, expected_args, overrides)
    blocks = settings_blocks(log_bytes.decode("utf-8", errors="replace"))
    if block_index is None:
        if len(blocks) != 1:
            raise ValueError(f"Expected exactly one Using Settings block, found {len(blocks)}; choose an explicit 1-based --settings-block")
        block_index = 1
    if not 1 <= block_index <= len(blocks):
        raise ValueError(f"--settings-block {block_index} is outside 1..{len(blocks)}")
    block = blocks[block_index - 1]
    production_args = source_arguments(source_bytes.decode("utf-8"), preset)
    settings = compare_settings(expected_fields, block["fields"])
    source_check = compare_arguments(expected_args, production_args)
    argv_check = compare_arguments(expected_args, block["argv"], allow_extra=True)
    argv_check["completeCommandLineSection"] = block["argvComplete"]
    argv_check["duplicateCommandLineStart"] = block.get("argvDuplicateStart", False)
    # These known independent overrides can invalidate a matching main option.
    argv_check["conflictingUnmanagedOverrides"] = [key for key in argv_check["unmanagedArguments"]
                                                  if key in ("-fxaamode", "-globallod")]
    settings["duplicateFields"] = block["duplicateFields"]
    problems = []
    for label, section in (("postApplySettings", settings), ("sourcePreset", source_check), ("runtimeArgv", argv_check)):
        for key in ("missing", "mismatches", "duplicateFields", "duplicateManagedArguments",
                    "duplicateExpectedArguments", "unexpectedArguments", "conflictingUnmanagedOverrides"):
            if section.get(key):
                problems.append(f"{label}.{key}")
    if not block["argvComplete"] or block.get("argvDuplicateStart"):
        problems.append("runtimeArgv.incompleteOrAmbiguousSection")
    for label, section in (("postApplySettings", settings), ("sourcePreset", source_check), ("runtimeArgv", argv_check)):
        section["passed"] = not any(problem.startswith(label + ".") for problem in problems)
    return {"schemaVersion": 1, "passed": not problems, "expectedPreset": preset,
            "scope": "Agreement of original post-ApplySettings fields, runtime argv and the supplied launcher source. This does not qualify FPS, pixels, shader/pass execution, HAP identity or device stability.",
            "inputs": {"log": artifact(log, log_bytes), "expectedPresets": artifact(presets, presets_bytes),
                       "gameQualitySource": artifact(source, source_bytes)},
            "settingsBlock": {"selectedOneBasedIndex": block_index, "availableBlocks": len(blocks),
                              "startLine": block["startLine"], "argvStartLine": block["argvStartLine"],
                              "argvEndLine": block.get("argvEndLine")},
            "postApplySettings": settings, "sourcePreset": source_check, "runtimeArgv": argv_check,
            "observedSettings": block["fields"], "expectedSettings": expected_fields,
            "productionArguments": production_args, "expectedArguments": expected_args,
            "expectationOverrides": changes,
            "failureCategories": problems,
            "candidateRevisionScope": "Expected values come from --presets, with no hardcoded high/medium/low field values. For tuned or historical candidates supply the matching expectation report and GameQuality.ets snapshot; do not relabel a source mismatch as a passed old phase."}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--expected", required=True, choices=("high", "medium", "low"))
    parser.add_argument("--log", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--presets", "--expected-report", type=Path, default=DEFAULT_PRESETS)
    parser.add_argument("--game-quality", type=Path, default=DEFAULT_SOURCE)
    parser.add_argument("--settings-block", type=int, help="Explicit 1-based block index for a log containing multiple launches")
    parser.add_argument("--override", action="append", default=[], metavar="FIELD=JSON_VALUE",
                        help="Tune an expected field and its CLI, e.g. ReflectionQuality=1; repeat for multiple fields")
    args = parser.parse_args()
    inputs = (args.log, args.presets, args.game_quality, Path(__file__))
    if args.out.resolve() in {path.resolve() for path in inputs} or (args.out.exists() and any(
            path.exists() and args.out.samefile(path) for path in inputs)):
        parser.error("--out must not overwrite any input or this script")
    try:
        result = check(args.log, args.presets, args.game_quality, args.expected, args.settings_block, args.override)
        exit_code = 0 if result["passed"] else 1
    except (OSError, ValueError, KeyError, TypeError) as error:
        result = {"schemaVersion": 1, "passed": False, "expectedPreset": args.expected,
                  "diagnosticError": str(error), "scope": "Input/parser failure; no quality, FPS or rendering claim."}
        exit_code = 2
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, ensure_ascii=False, indent=2, allow_nan=False) + "\n")
    print(json.dumps({"passed": result["passed"], "report": str(args.out.resolve()),
                      "failureCategories": result.get("failureCategories", []),
                      "diagnosticError": result.get("diagnosticError")}, ensure_ascii=False))
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
