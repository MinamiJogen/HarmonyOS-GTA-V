#!/usr/bin/env python3
"""Real core trackers with real core NOOP resources, in a disposable source copy.

Never edits workspace vendor code. Temporarily enables only newly added OHOS
draft declarations on the host, adds read-only fingerprints, and runs the actual
set_from_usage_scope/drain/provider/skip_barrier/metadata chain. NOOP HAL does
not validate Vulkan submission/barriers or any physical GPU behavior.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time


def fingerprint(path):
    return {'path': str(path.resolve()), 'sha256': hashlib.sha256(path.read_bytes()).hexdigest()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--worktree', type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    draft = root / 'evidence/native-30fps-800p-20261021/empty-render-prepass-draft'
    worktree = args.worktree or Path(tempfile.mkdtemp(prefix='gta-real-tracker-'))
    core = worktree / 'game-runtime/vendor/wgpu-core'
    source = root / 'game-runtime/vendor/wgpu-core'
    module_relative = 'src/command/empty_render_prepass.rs'
    presence = [(source / module_relative).is_file()]
    presence += [all(name in (source / relative).read_text() for name in
                     ('fn scope_proves_no_transitions(', 'fn pending_transitions_are_empty('))
                 for relative in ('src/track/buffer.rs', 'src/track/texture.rs')]
    presence += ['use super::empty_render_prepass::' in (source / 'src/command/render.rs').read_text(),
                 'pub(crate) mod empty_render_prepass;' in (source / 'src/command/mod.rs').read_text()]
    if any(presence) and not all(presence):
        raise ValueError('production prepass integration is incomplete; wait for source freeze')
    applied = all(presence)
    if worktree.resolve() == root or root in worktree.resolve().parents:
        raise ValueError('host source copy must be outside the workspace')
    if core.exists():
        core.rename(worktree / ('previous-core-source-' + str(time.monotonic_ns())))
    (worktree / '.cargo').mkdir(exist_ok=True)
    shutil.copyfile(root / 'game-runtime/.cargo/config.toml', worktree / '.cargo/config.toml')
    if not core.exists():
        shutil.copytree(source, core, ignore=shutil.ignore_patterns('target', '.git'))
        if not applied:
            patch = (draft / 'proposed-vendor.patch').read_text()
            # Host override of newly added declarations only.
            patch = '\n'.join(line.replace('all(target_env = "ohos", feature = "std")',
                                           'feature = "std"')
                              if line.startswith('+') else line
                              for line in patch.splitlines()) + '\n'
            subprocess.run(['patch', '--batch', '-p1'], cwd=worktree,
                           input=patch, text=True, check=True, capture_output=True)
        else:
            # Production is already patched. Never apply the draft again or
            # replace its helper with a potentially older draft. Broaden only
            # the six declarations/blocks introduced by this feature in temp.
            guard = r'#\[cfg\(all\(target_env = "ohos", feature = "std"\)\)\]'
            replacements = {
                'src/command/mod.rs': (guard + r'(?=\n\s*pub\(crate\) mod empty_render_prepass;)', 1),
                'src/track/buffer.rs': (guard + r'(?=\n\s*pub\(crate\) fn (?:pending_transitions_are_empty|scope_proves_no_transitions)\()', 2),
                'src/track/texture.rs': (guard + r'(?=\n\s*pub\(crate\) fn (?:pending_transitions_are_empty|scope_proves_no_transitions)\()', 2),
                'src/command/render.rs': (guard + r'(?=\n\s*\{\n\s*use super::empty_render_prepass::)', 1),
            }
            for relative, (pattern, expected) in replacements.items():
                target = core / relative
                text, count = re.subn(pattern, '#[cfg(feature = "std")]', target.read_text())
                if count != expected:
                    raise ValueError(f'applied source host-cfg drift: {relative}: {count} != {expected}')
                target.write_text(text)
        with (core / 'src/track/mod.rs').open('a') as f:
            f.write('\n#[cfg(all(test, feature="noop"))]\nmod host_tracker_harness;\n')
    if not applied:
        shutil.copyfile(draft / 'empty_render_prepass.rs', core / module_relative)
    shutil.copyfile(draft / 'host_tracker_harness.rs', core / 'src/track/host_tracker_harness.rs')
    for name, states in [('buffer', '&self.start, &self.end'),
                         ('texture', '&self.start_set, &self.end_set')]:
        tracker = name.title() + 'Tracker'
        path = core / f'src/track/{name}.rs'
        original_code = path.read_text().split('// TEMPORARY HOST TEST INTROSPECTION ONLY.')[0]
        extra = '''
// TEMPORARY HOST TEST INTROSPECTION ONLY. No production tracker body changes.
#[cfg(test)]
impl ''' + tracker + ''' {
    pub(super) fn host_fingerprint(&self) -> alloc::string::String {
        let owned: Vec<_> = self.metadata.owned_indices().map(|i| {
            let r = unsafe { self.metadata.get_resource_unchecked(i) };
            (i, Arc::as_ptr(r) as usize)
        }).collect();
        alloc::format!("{:?}|{:?}|{:?}|{:?}", ''' + states + ''', owned, self.temp)
    }
}
'''
        if name == 'texture':
            extra += '''
#[cfg(test)]
impl TextureTracker {
    pub(super) fn host_force_end_sentinel(&mut self, texture: &Texture, state: TextureUses) {
        let index = texture.tracker_index().as_usize();
        self.end_set.simple[index] = state;
        self.end_set.complex.remove(&index);
    }
}
'''
        path.write_text(original_code + extra)
    # Root-crate resolution considers all optional/platform edges. std, noop
    # and wgsl are selected; wgsl is required by real device indirect validation.
    # Prune inactive declarations
    # rather than fetching unrelated serde/browser/backend dependencies.
    manifest_path = core / 'Cargo.toml'
    # Always derive from the original manifest so reruns are deterministic.
    original = (source / 'Cargo.toml').read_text()
    feature_block = re.search(r'\[features\]\n(.*?)(?=\n\[)', original, re.S)[1]
    feature_names = re.findall(r'^([A-Za-z0-9_-]+)\s*=', feature_block, re.M)
    pruned = []
    def remove_optional(match):
        if re.search(r'^optional\s*=\s*true\s*$', match[0], re.M):
            pruned.append(match[1])
            return '\n'
        return match[0]
    manifest = re.sub(r'\n\[([^\n]*dependencies[^\n]*)\]\n.*?(?=\n\[|\Z)',
                      remove_optional, original, flags=re.S)
    inactive_features = '\n'.join(name + ' = []' for name in feature_names
                                  if name not in ('default', 'strict_asserts', 'wgsl'))
    features = '[features]\ndefault = ["std"]\nstrict_asserts = ["wgpu-types/strict_asserts"]\nwgsl = ["naga/wgsl-in"]\n' + inactive_features + '\n'
    manifest = re.sub(r'\[features\]\n.*?(?=\n\[)', features, manifest, flags=re.S)
    manifest += '\n[patch.crates-io]\nwgpu-hal = { path = ' + json.dumps(str(root / 'game-runtime/vendor/wgpu-hal')) + ' }\n'
    manifest_path.write_text(manifest)
    shutil.copyfile(root / 'game-runtime/Cargo.lock', core / 'Cargo.lock')
    prefix = 'host-tracker-applied' if applied else 'host-tracker'
    log = draft / (prefix + '-tests.log')
    command = ['cargo', 'test', '--offline', '--manifest-path', str(core / 'Cargo.toml'),
               '--target-dir', str(worktree / 'target'), '--lib', '--no-default-features',
               '--features', 'std,noop,wgsl', 'host_tracker_harness', '--', '--nocapture']
    with log.open('w') as output:
        done = subprocess.run(command, cwd=worktree, stdout=output, stderr=subprocess.STDOUT)
    text = log.read_text()
    if applied and fingerprint(core / module_relative)['sha256'] != fingerprint(source / module_relative)['sha256']:
        raise ValueError('compiled production helper changed from input')
    sources = ['src/track/mod.rs', 'src/track/buffer.rs', 'src/track/texture.rs',
               'src/track/metadata.rs', 'src/command/render.rs']
    report = {
        'schemaVersion': 1, 'createdAt': datetime.now(timezone.utc).isoformat(),
        'status': 'pass' if done.returncode == 0 and '8 passed; 0 failed' in text else 'failed',
        'exitCode': done.returncode, 'workspaceVendorEdited': False,
        'sourceMode': 'applied_production_copy_no_patch_or_helper_replacement' if applied else 'base_copy_with_draft_patch',
        'draftPatchApplied': not applied,
        'productionHelperCopiedByteForByte': applied,
        'compiledHelper': fingerprint(core / module_relative),
        'inactiveOptionalDeclarationsPrunedOnlyInTemp': pruned,
        'worktree': str(worktree), 'command': command,
        'inputs': [fingerprint(source / p) for p in sources] +
                  [fingerprint(draft / 'proposed-vendor.patch'),
                   fingerprint((source if applied else draft) / (module_relative if applied else 'empty_render_prepass.rs')),
                   fingerprint(draft / 'host_tracker_harness.rs')],
        'log': fingerprint(log), 'expectedHostTests': 8,
        'compiledHostHarness': 'running 8 tests' in text,
        'executedHostTests': len(re.findall(r'^test track::host_tracker_harness::.* \.\.\. (?:ok|FAILED)$', text, re.M)),
        'actualCode': 'Complete original core tracker set_from_usage_scope, insert/update/barrier/provider/skip_barrier, metadata ownership and drain code. Patched read-only proof inspector is compiled and called. NOOP core resources created through real Global/adapter/device APIs.',
        'temporaryChanges': ('Applied production source copied with no draft patch and no helper replacement; ' if applied else 'Draft patch applied only to temp source copy; ') + 'six new OHOS declarations enabled on host; read-only fingerprints and test module appended only in temp. Inactive optional/platform manifest edges pruned for offline resolution; inactive feature definitions made empty. Selected std/noop empty edges and wgsl=naga/wgsl-in remain original. WGSL required by actual device indirect validation initialization. No tracker algorithm is mirrored or replaced.',
        'coverage': ['fresh simple resource CPU adoption without local barrier',
                     'equal ordered states', 'equal unordered writes', 'changed usage',
                     'existing pending transitions', 'new complex texture', 'owned complex texture',
                     'UNKNOWN/UNINITIALIZED read-only proof rejection with test-only sentinel injection'],
        'limits': 'NOOP HAL resource operations are GPU stubs. No full render-pass caller, Vulkan barrier encoding/submission, driver/device pixel/performance, or ALL possible tracker state proof is established. Differential tests compare real CPU state/ownership/pending/drained transitions on fixtures.',
    }
    (draft / (prefix + '-validation.json')).write_text(json.dumps(report, ensure_ascii=False, indent=2) + '\n')
    print(json.dumps({'status': report['status'], 'worktree': str(worktree), 'log': str(log)}, indent=2))
    raise SystemExit(done.returncode or (0 if report['status'] == 'pass' else 1))


if __name__ == '__main__':
    main()
