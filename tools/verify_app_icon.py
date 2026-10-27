#!/usr/bin/env python3
"""Verify the packaged layered app icon against both source resource copies."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import zipfile


def _resource_reference_matches(value, name, extension):
    return isinstance(value, str) and (
        value == "$media:" + name
        or value.endswith("/media/" + name + "." + extension)
    )


def verify_app_icon(package, repo):
    """Return icon report fields for an open HAP ZipFile, failing on mismatch."""
    source_roots = (
        repo / "harmony/AppScope/resources/base/media",
        repo / "harmony/entry/src/main/resources/base/media",
    )

    def checked_resource(filename):
        member = "resources/base/media/" + filename
        payload = package.read(member)
        for source_root in source_roots:
            source = source_root / filename
            assert payload == source.read_bytes(), "Packaged icon resource differs from source: " + str(source)
        return payload

    composite = checked_resource("app_icon_gta5.png")
    descriptor_name = "app_icon_gta5_layered.json"
    source_descriptor = (source_roots[0] / descriptor_name).read_bytes()
    assert source_descriptor == (source_roots[1] / descriptor_name).read_bytes(), "AppScope and entry layered icon descriptors differ"
    expected_layers = {
        "background": "$media:app_icon_gta5_background",
        "foreground": "$media:app_icon_gta5_foreground",
    }
    assert json.loads(source_descriptor) == {"layered-image": expected_layers}, "Unexpected source layered app icon descriptor"
    descriptor = package.read("resources/base/media/" + descriptor_name)
    layered_image = json.loads(descriptor)
    assert isinstance(layered_image, dict) and set(layered_image) == {"layered-image"}, "Unexpected packaged layered app icon descriptor"
    packaged_layers = layered_image["layered-image"]
    assert isinstance(packaged_layers, dict) and set(packaged_layers) == set(expected_layers), "Unexpected packaged layered app icon layers"
    background = checked_resource("app_icon_gta5_background.png")
    foreground = checked_resource("app_icon_gta5_foreground.png")
    deveco = Path(os.environ.get("DEVECO_CONTENTS", "/Applications/DevEco-Studio.app/Contents"))
    restool = deveco / "sdk/default/openharmony/toolchains/restool"
    assert restool.is_file(), "Official resource index reader is unavailable: " + str(restool)
    resource_index = json.loads(subprocess.check_output([str(restool), "dump", str(package.filename)], text=True))
    layer_resources = {}
    for layer, source_reference in expected_layers.items():
        name = source_reference.removeprefix("$media:")
        resources = [resource for resource in resource_index["resource"]
                     if resource.get("type") == "media" and resource.get("name") == name]
        assert len(resources) == 1, "Layer resource is missing or ambiguous in the packaged index: " + name
        resource = resources[0]
        member = "resources/base/media/" + name + ".png"
        paths = [entry.get("value") for entry in resource.get("entryValues", [])]
        expected_paths = {member, resource_index["moduleName"] + "/" + member}
        assert paths and all(path in expected_paths for path in paths), "Layer resource index points to an unexpected image: " + name
        reference = packaged_layers[layer]
        assert (reference == source_reference or reference == "$media:" + str(resource["id"])
                or _resource_reference_matches(reference, name, "png")), "Packaged layer reference does not match its resource index: " + layer
        layer_resources[layer] = {"id": resource["id"], "name": name, "paths": paths, "reference": reference}
    module = json.loads(package.read("module.json"))
    app_reference = module["app"].get("icon")
    assert _resource_reference_matches(app_reference, "app_icon_gta5_layered", "json"), "Packaged app does not reference the layered icon"
    abilities = module["module"].get("abilities", [])
    ability_references = {ability["name"]: ability.get("icon") for ability in abilities}
    assert {"EntryAbility", "GraphicsFrameProbeAbility"} <= ability_references.keys(), "Expected application abilities are missing"
    for ability in abilities:
        assert _resource_reference_matches(ability.get("icon"), "app_icon_gta5_layered", "json"), "Packaged ability does not reference the layered icon: " + ability["name"]
        assert _resource_reference_matches(ability.get("startWindowIcon"), "app_icon_gta5", "png"), "Packaged start window does not reference the composite icon: " + ability["name"]
    return {
        "selectedAppIconMatches": True,
        "appIconSha256": hashlib.sha256(composite).hexdigest(),
        "layeredIcon": {
            "descriptor": "resources/base/media/app_icon_gta5_layered.json",
            "descriptorSha256": hashlib.sha256(descriptor).hexdigest(),
            "sourceDescriptorSha256": hashlib.sha256(source_descriptor).hexdigest(),
            "foregroundSha256": hashlib.sha256(foreground).hexdigest(),
            "backgroundSha256": hashlib.sha256(background).hexdigest(),
            "sourceCopiesMatch": True,
            "packagedResourcesMatch": True,
            "compiledDescriptorMatches": True,
            "resourceIndexChecked": True,
            "layers": layer_resources,
            "appIconReference": app_reference,
            "abilityIconReferences": ability_references,
        },
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--hap", required=True, type=Path, help="HAP package to inspect")
    parser.add_argument("--report", type=Path, help="Optional JSON report output path")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    with zipfile.ZipFile(args.hap) as package:
        assert package.testzip() is None, "HAP ZIP CRC failure"
        report = {
            "status": "PASS_layered_app_icon_static_verification",
            "hapFile": str(args.hap.resolve()),
            **verify_app_icon(package, repo),
            "scope": "Static package icon verification only; launcher rendering is not established.",
        }
    output = json.dumps(report, indent=2, ensure_ascii=False) + "\n"
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(output)
    print(output, end="")


if __name__ == "__main__":
    main()
