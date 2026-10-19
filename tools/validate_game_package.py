#!/usr/bin/env python3
"""Read-only Muguet game ZIP inventory; no extraction or game execution.

Usage: python3 tools/validate_game_package.py /path/to/game.zip [--hash-wasm]
JSON goes to stdout. Exit 0 means this limited preflight passed, 1 means failed,
and 2 means a command-line error. This does not establish runtime compatibility.

Structure follows Muguet src/import.rs, src/config.rs and src/gpu.rs.
Hook names follow the upstream src/patch.rs schema with VERSION=11.
Only ZIP member metadata, bounded JSON files and 8 Wasm header bytes are read by
default; --hash-wasm additionally decompresses each selected Wasm in memory-sized
chunks to compute SHA-256, without writing the bytes to disk.
"""

import argparse
import hashlib
import json
import re
import stat
import sys
import zipfile
import zlib
from pathlib import Path


SKIPPED = {"env_test", "verityradio"}
HOOK_REQUIREMENTS = [
    ("muguet_script", "GtaThread::Update(int)"),
    ("muguet_explosion", "CExplosionManager::AddExplosion(CExplosionManager::CExplosionArgs&, CProjectile*, bool)"),
    ("muguet_timewarp", "CTimeWarper::SetTargetTimeWarp(TW_Dir)"),
    ("muguet_text", "CTextFile::GetInternal(unsigned int)"),
    ("muguet_txd", "graphics_commands::HasStreamedTxdLoaded(char const*)"),
    ("muguet_pool", "rage::sysMemGrowBuddyAllocator::GrowHeap()"),
    ("muguet_cash", "stats_commands::CommandStatSetInt(int, int, bool)"),
    ("muguet_opus_frame", "opus_decode_frame"),
    ("muguet_resource_map", "rage::datResourceInfo::GenerateMap(rage::datResourceMap&) const"),
]
READ_ERRORS = (OSError, RuntimeError, ValueError, NotImplementedError,
               zipfile.BadZipFile, EOFError, zlib.error, RecursionError)


def reject_json_constant(value):
    raise ValueError(f"invalid non-finite JSON constant: {value}")


def path_problem(name):
    """Use portable rejection rules even on a POSIX host; never extract names."""
    if "\0" in name:
        return "NUL byte in member name"
    portable = name.replace("\\", "/")
    if portable.startswith("/") or re.match(r"^[A-Za-z]:", portable):
        return "absolute or drive-qualified path"
    parts = portable.rstrip("/").split("/")
    if ".." in parts:
        return "parent traversal component (..)"
    if not name or any(part in {"", "."} for part in parts):
        return "empty or noncanonical path component"
    if "\\" in name:
        return "backslash path is ambiguous across import platforms"
    return None


class Audit:
    def __init__(self, path, hash_wasm=False, max_json_bytes=32 << 20):
        self.path = path
        self.hash_wasm = hash_wasm
        self.max_json_bytes = max_json_bytes
        self.report = {
            "schema_version": 1,
            "package": str(path.absolute()),
            "scope": "ZIP structure and selected resource metadata only; does not prove game execution or HarmonyOS compatibility",
            "validation": {
                "all_member_crc": "not_verified; only members read to EOF have their ZIP CRC checked",
                "wasm_module_semantics": "not_verified; magic and version only",
                "shader_compilation": "not_verified",
                "full_data_content": "not_verified; manifest references are checked for presence only",
            },
            "hooks": {
                "status": "not_verified",
                "reason": "Wasm name/import sections are not parsed; patch compatibility and already-patched status remain unverified",
                "upstream_patch_version": 11,
                "required_function_names": [
                    {"hook": hook, "function": function}
                    for hook, function in HOOK_REQUIREMENTS
                ] + [{"function": "rage::sysMemGrowBuddyAllocator::GetHeapSize() const", "reason": "required by pool hook rewriting"}],
                "tls_global": "__tls_base; upstream falls back to index 1 if absent (not verified here)",
            },
            "errors": [],
            "warnings": [],
            "key_files": [],
            "builds": [],
        }
        self.members = {}

    def issue(self, code, message, member=None, warning=False):
        item = {"code": code, "message": message}
        if member is not None:
            item["member"] = member
        self.report["warnings" if warning else "errors"].append(item)

    def read_json(self, archive, name):
        info = self.members[name]
        if info.file_size > self.max_json_bytes:
            self.issue("json_size_limit", f"JSON exceeds bounded read limit ({self.max_json_bytes} bytes); increase --max-json-mib if expected", name)
            return None
        try:
            with archive.open(info) as stream:
                raw = stream.read(self.max_json_bytes + 1)
            if len(raw) > self.max_json_bytes:
                self.issue("json_size_limit", "JSON expanded beyond bounded read limit", name)
                return None
            entry = self.file_metadata(info)
            entry["sha256"] = hashlib.sha256(raw).hexdigest()
            self.report["key_files"].append(entry)
            return json.loads(raw.decode("utf-8"), parse_constant=reject_json_constant)
        except UnicodeDecodeError as exc:
            self.issue("invalid_json_encoding", str(exc), name)
        except json.JSONDecodeError as exc:
            self.issue("invalid_json", f"{exc.msg} at line {exc.lineno}, column {exc.colno}", name)
        except READ_ERRORS as exc:
            self.issue("member_read_failed", f"{type(exc).__name__}: {exc}", name)
        return None

    @staticmethod
    def file_metadata(info):
        return {"member": info.filename, "size_bytes": info.file_size,
                "compressed_bytes": info.compress_size, "zip_crc32": f"{info.CRC:08x}"}

    def scan_members(self, archive):
        infos = archive.infolist()
        self.report["archive"] = {
            "size_bytes": self.path.stat().st_size,
            "entry_count": len(infos),
            "uncompressed_bytes": sum(i.file_size for i in infos),
            "compressed_bytes": sum(i.compress_size for i in infos),
        }
        folded = {}
        for info in infos:
            original = info.orig_filename
            problem = path_problem(original)
            if problem:
                self.issue("unsafe_member_path", problem, original)
                continue
            if info.filename in self.members:
                self.issue("duplicate_member", "duplicate ZIP member is ambiguous", info.filename)
                continue
            mode = info.external_attr >> 16
            kind = stat.S_IFMT(mode)
            if stat.S_ISLNK(mode):
                self.issue("symlink_member", "symbolic link member is not allowed", info.filename)
                continue
            if kind not in {0, stat.S_IFREG, stat.S_IFDIR}:
                self.issue("special_member", f"unsupported Unix file type: {kind:o}", info.filename)
                continue
            if kind == stat.S_IFDIR and not info.is_dir():
                self.issue("directory_type_mismatch", "Unix directory mode lacks a trailing slash in the ZIP name", info.filename)
                continue
            if info.flag_bits & 1:
                self.issue("encrypted_member", "encrypted entries cannot be imported without a password", info.filename)
            if info.filename.casefold() in folded:
                self.issue("case_collision", f"case-insensitive collision with {folded[info.filename.casefold()]}", info.filename, warning=True)
            folded[info.filename.casefold()] = info.filename
            self.members[info.filename] = info
        for name in self.members:
            parts = name.rstrip("/").split("/")
            for count in range(1, len(parts)):
                parent = "/".join(parts[:count])
                if self.file_present(parent):
                    self.issue("file_directory_conflict", f"ancestor {parent} is a file, so this path cannot be imported", name)

    def file_present(self, name):
        info = self.members.get(name)
        return info is not None and not info.is_dir()

    def manifest(self, archive, name, prefix):
        manifest = self.read_json(archive, name)
        if manifest is None:
            return
        if not isinstance(manifest, dict):
            self.issue("manifest_type", "manifest must be a JSON object", name)
            return
        files = manifest.get("files")
        if not isinstance(files, list):
            self.issue("manifest_files_unverified", "no array-valued files field; upstream retains the object, but reference completeness cannot be verified", name, warning=True)
            return
        summary = {"entry_count": len(files), "present": 0, "missing": [], "upstream_skipped": []}
        self.report["manifest_files"] = summary
        seen = set()
        for index, row in enumerate(files):
            if not isinstance(row, list) or not row or not isinstance(row[0], str):
                self.issue("manifest_file_entry", f"files[{index}] must be an array whose first element is a path string", name)
                continue
            relative = row[0]
            problem = path_problem(relative)
            if problem or relative.endswith("/"):
                self.issue("unsafe_manifest_path", f"files[{index}]: {problem or 'directory instead of file'}", relative)
                continue
            if relative in seen:
                self.issue("duplicate_manifest_path", "duplicate file reference", relative, warning=True)
            seen.add(relative)
            if any(part.lower() in SKIPPED for part in relative.split("/")):
                summary["upstream_skipped"].append(relative)
            elif self.file_present(prefix + "data/" + relative):
                summary["present"] += 1
            else:
                summary["missing"].append(relative)
        if summary["missing"]:
            self.issue("missing_manifest_files", f"{len(summary['missing'])} referenced data files missing; upstream tidy_lists would silently remove these references", name)

    def wasm(self, archive, name):
        info = self.members[name]
        entry = self.file_metadata(info)
        try:
            with archive.open(info) as stream:
                header = stream.read(8)
                entry["header_hex"] = header.hex()
                entry["magic_valid"] = header[:4] == b"\0asm"
                entry["version"] = int.from_bytes(header[4:8], "little") if len(header) == 8 else None
                if len(header) != 8 or not entry["magic_valid"] or entry["version"] != 1:
                    self.issue("invalid_wasm_header", "expected WebAssembly magic 0061736d and core version 1", name)
                if self.hash_wasm:
                    digest = hashlib.sha256(header)
                    read_bytes = len(header)
                    while chunk := stream.read(1 << 20):
                        digest.update(chunk)
                        read_bytes += len(chunk)
                    if read_bytes != info.file_size:
                        self.issue("wasm_size_mismatch", f"read {read_bytes} bytes; ZIP reports {info.file_size}", name)
                    entry["sha256"] = digest.hexdigest()
                else:
                    entry["sha256"] = None
                    entry["sha256_status"] = "not_requested; use --hash-wasm for a streaming full-content hash"
        except READ_ERRORS as exc:
            self.issue("member_read_failed", f"{type(exc).__name__}: {exc}", name)
        self.report["key_files"].append(entry)

    def shaders(self, archive, name):
        index = self.read_json(archive, name)
        if index is None:
            return None
        if not isinstance(index, dict):
            self.issue("shader_index_type", "shader index must be a hash-to-record JSON object", name)
            return None
        summary = {"entry_count": len(index), "ok_entries": 0, "non_ok_entries": 0,
                   "required_wgsl_present": 0, "required_consts_present": 0, "missing": []}
        base = name[:-len("index.json")]
        if not index:
            self.issue("empty_shader_index", "shader index has no entries", name)
        for key, record in index.items():
            if key == "_packs":
                # Browser shader indices also contain a pack metadata table.
                # Native Muguet looks up shader hashes directly and ignores it.
                if not isinstance(record, list):
                    self.issue("shader_pack_metadata", "_packs must be an array", name)
                else:
                    summary["pack_metadata_entries"] = len(record)
                    for pack in record:
                        filename = pack.get("file") if isinstance(pack, dict) else None
                        if not isinstance(filename, str) or path_problem(filename) or "/" in filename:
                            self.issue("shader_pack_metadata", "pack file must be a safe filename", name)
                continue
            if path_problem(key) or "/" in key or key.endswith("/"):
                self.issue("unsafe_shader_key", "shader hash key must be a single safe filename component", key)
                continue
            if not isinstance(record, dict):
                self.issue("shader_record_type", f"shader entry {key} must be an object", name)
                continue
            if record.get("ok") is not True:
                summary["non_ok_entries"] += 1
                continue
            summary["ok_entries"] += 1
            required = [(key + ".wgsl", "required_wgsl_present")]
            # Mirror serde_json semantics: 0, empty strings/arrays still request consts.
            if "consts" in record and record["consts"] is not None and record["consts"] is not False:
                required.append((key + ".consts.json", "required_consts_present"))
            for filename, counter in required:
                member = base + filename
                if not self.file_present(member):
                    summary["missing"].append(member)
                    continue
                summary[counter] += 1
                if filename.endswith(".consts.json"):
                    consts = self.read_json(archive, member)
                    if consts is not None and not isinstance(consts, list):
                        self.issue("shader_consts_type", "shader constants must be a JSON array", member)
                elif self.members[member].file_size == 0:
                    self.issue("empty_shader", "required WGSL file is empty", member)
        if summary["missing"]:
            self.issue("missing_shader_files", f"{len(summary['missing'])} shader files required by the index are absent", name)
        if summary["non_ok_entries"]:
            self.issue("unavailable_shaders", f"{summary['non_ok_entries']} index entries have no ok:true shader; impact depends on game usage", name, warning=True)
        if summary["ok_entries"] == 0:
            self.issue("no_usable_shader_entries", "shader index has no ok:true records", name)
        return summary

    def run(self):
        try:
            with zipfile.ZipFile(self.path) as archive:
                self.scan_members(archive)
                manifests = [n for n in self.members if self.file_present(n)
                             and (n == "data/manifest.json" or n.endswith("/data/manifest.json"))]
                if not manifests:
                    self.issue("missing_manifest", "no data/manifest.json; provide a ZIP whose root or nested prefix holds data/ and b/<build>/")
                    return self.finish()
                # min preserves ZIP order for equal-length names, as upstream does.
                name = min(manifests, key=len)
                prefix = name[:-len("data/manifest.json")]
                self.report["layout"] = {"selected_prefix": prefix, "manifest_candidates": manifests,
                                         "prefix_selection": "shortest manifest path; equal lengths retain ZIP member order (upstream import.rs)"}
                if len(manifests) > 1:
                    self.issue("multiple_manifests", f"selected {name}; other manifest trees are ignored by upstream", warning=True)
                self.manifest(archive, name, prefix)
                builds = {}
                for member in self.members:
                    if not member.startswith(prefix + "b/"):
                        continue
                    relative = member[len(prefix + "b/"):]
                    if "/" not in relative:
                        continue
                    build, inner = relative.split("/", 1)
                    if inner in {"game.wasm", "shaders/index.json"} and self.file_present(member):
                        builds.setdefault(build, {})[inner] = member
                for build, found in sorted(builds.items()):
                    item = {"build": build, "wasm": found.get("game.wasm"),
                            "shader_index": found.get("shaders/index.json")}
                    self.report["builds"].append(item)
                    if "game.wasm" in found:
                        self.wasm(archive, found["game.wasm"])
                    if "shaders/index.json" in found:
                        item["shaders"] = self.shaders(archive, found["shaders/index.json"])
                    if len(found) != 2:
                        self.issue("incomplete_build", "a direct b/<build>/ needs game.wasm and shaders/index.json together", prefix + "b/" + build + "/")
                if not builds:
                    self.issue("missing_build", "no direct b/<build>/game.wasm or shaders/index.json under the selected prefix")
                if len(builds) > 1:
                    self.issue("multiple_builds", "multiple build trees; upstream runtime chooses an unsorted directory entry, so pair selection is ambiguous", warning=True)
                selected = [info for member, info in self.members.items()
                            if member.startswith(prefix) and self.upstream_wanted(member[len(prefix):])]
                self.report["upstream_import"] = {"keep_code": True, "selected_file_count": len(selected),
                                                  "selected_uncompressed_bytes": sum(i.file_size for i in selected),
                                                  "skipped_directory_components": sorted(SKIPPED)}
        except (OSError, zipfile.BadZipFile, zipfile.LargeZipFile, ValueError, EOFError) as exc:
            self.issue("archive_open_failed", f"{type(exc).__name__}: {exc}")
        return self.finish()

    @staticmethod
    def upstream_wanted(relative):
        if relative.endswith("/") or any(p.lower() in SKIPPED for p in relative.split("/")):
            return False
        if relative.startswith("data/"):
            return True
        if relative.startswith("b/"):
            inner = relative[2:].partition("/")[2]
            return inner == "game.wasm" or inner.startswith("shaders/")
        return False

    def finish(self):
        self.report["status"] = "pass" if not self.report["errors"] else "fail"
        return self.report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("package", type=Path, help="existing game ZIP; read only")
    parser.add_argument("--hash-wasm", action="store_true", help="stream entire selected Wasm members into SHA-256 (may take time)")
    parser.add_argument("--max-json-mib", type=int, default=32, help="per-JSON bounded read limit in MiB (default: 32)")
    args = parser.parse_args(argv)
    if args.max_json_mib < 1:
        parser.error("--max-json-mib must be at least 1")
    report = Audit(args.package, args.hash_wasm, args.max_json_mib << 20).run()
    json.dump(report, sys.stdout, ensure_ascii=False, indent=2)
    sys.stdout.write("\n")
    return 0 if report["status"] == "pass" else 1


if __name__ == "__main__":
    sys.exit(main())
