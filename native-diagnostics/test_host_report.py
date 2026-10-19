#!/usr/bin/env python3
"""Verify JSON, loader errors and synthetic enumeration on macOS; no Vulkan GPU."""
import json
import ctypes
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent
SDK = Path(os.environ.get("OHOS_SDK_NATIVE", "/Applications/DevEco-Studio.app/Contents/sdk/default/openharmony/native"))

with tempfile.TemporaryDirectory(prefix="muguet-vulkan-host-") as temporary:
    temp = Path(temporary)
    binary, library = temp / "probe", temp / "synthetic-vulkan.dylib"
    cabi_library = temp / "probe-cabi.dylib"
    common = ["/usr/bin/clang++", "-std=c++17", "-O2", "-Wall", "-Wextra", "-Werror", "-idirafter", str(SDK / "sysroot/usr/include")]
    subprocess.run(common + ["-DMUGUET_VULKAN_PROBE_STANDALONE=1", str(ROOT / "src/vulkan_probe.cpp"), "-o", str(binary)], check=True)
    subprocess.run(common + ["-dynamiclib", str(ROOT / "src/fake_vulkan_host_test.cpp"), "-o", str(library)], check=True)
    subprocess.run(common + ["-dynamiclib", str(ROOT / "src/vulkan_probe.cpp"), "-o", str(cabi_library)], check=True)
    scenarios = [
        ("missing-loader", temp / "missing.so", {}, 1, "dlopen"),
        ("synthetic-enumeration", library, {}, 0, None),
        ("synthetic-zero-devices", library, {"MUGUET_VULKAN_FIXTURE_EMPTY": "1"}, 1, "vkEnumeratePhysicalDevices"),
    ]
    for name, loader, extra_env, exit_code, error_stage in scenarios:
        result = subprocess.run([str(binary), "--loader", str(loader)], capture_output=True, text=True, env={**os.environ, **extra_env})
        report = json.loads(result.stdout)
        assert result.returncode == exit_code, (name, result)
        assert report["status"] == ("pass" if exit_code == 0 else "fail"), name
        assert report["ohos_surface"]["surface_created"] is False
        assert report["ohos_surface"]["presentation_verified"] is False
        if error_stage:
            assert report["errors"][0]["stage"] == error_stage, name
        else:
            gpu = report["physical_devices"][0]
            assert "SYNTHETIC HOST FIXTURE" in gpu["name"]
            assert len(gpu["core_features"]) == 55
            assert len(gpu["compressed_formats"]) == 44
            assert gpu["memory_heaps"][0]["size_bytes"] == 1 << 20
            astc = next(f for f in gpu["compressed_formats"] if f["format"] == "VK_FORMAT_ASTC_4x4_UNORM_BLOCK")
            bc = next(f for f in gpu["compressed_formats"] if f["format"] == "VK_FORMAT_BC1_RGB_UNORM_BLOCK")
            assert astc["optimal_sampled_image"] is True
            assert bc["optimal_sampled_image"] is False
            assert report["ohos_surface"]["extension_advertised"] is True
            assert report["ohos_surface"]["create_surface_entry_available"] is False
        (ROOT / "logs" / ("host-" + name + ".json")).write_text(json.dumps(report, indent=2) + "\n")
        print(name + ": passed")
    cabi = ctypes.CDLL(str(cabi_library))
    cabi.muguet_vulkan_probe_json.restype = ctypes.c_char_p
    cabi_report = json.loads(cabi.muguet_vulkan_probe_json())
    assert cabi_report["probe"] == "ohos_vulkan_capabilities"
    assert cabi_report["ohos_surface"]["surface_created"] is False
    assert cabi_report["ohos_surface"]["presentation_verified"] is False
    print("c-abi-json-export: passed")
print("All 4 host report/ABI scenarios passed; no device/GPU result claimed.")
