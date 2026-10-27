#!/usr/bin/env python3
"""Run the production PSO JSON store with real serde/wgpu texture-format types.

Only a temporary Cargo host crate is built. No GPU, device, driver cache bytes,
or workspace source mutations are involved; these are storage/schema tests.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile


HARNESS = r'''
#[cfg(test)]
mod host_tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::thread;

    static ROOT_ID: AtomicU64 = AtomicU64::new(0);
    struct Root(PathBuf);
    impl Root {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("pso-host-{}-{}", std::process::id(),
                ROOT_ID.fetch_add(1, Ordering::Relaxed)));
            fs::create_dir(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
            Self(path)
        }
        fn store(&self, read_only: bool) -> Store { Store::new(&self.0, "current-build-and-shaders".into(), read_only) }
        fn file(&self) -> PathBuf { self.0.join(DIRECTORY).join(FILENAME) }
        fn install(&self, bytes: &[u8]) {
            fs::create_dir_all(self.0.join(DIRECTORY)).unwrap();
            fs::set_permissions(self.0.join(DIRECTORY), fs::Permissions::from_mode(0o700)).unwrap();
            fs::write(self.file(), bytes).unwrap();
            fs::set_permissions(self.file(), fs::Permissions::from_mode(0o600)).unwrap();
        }
    }
    impl Drop for Root { fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); } }
    fn recipe() -> Recipe {
        Recipe { vs: Shader { hash: "0123456789abcdef".into(), pause: false, source_sha256: "a".repeat(64) },
            ps: Some(Shader { hash: "fedcba9876543210".into(), pause: true, source_sha256: "b".repeat(64) }),
            signature: vec![[1, 2, 3]], elements: vec![[1, 2, 28, 0, 16, 0]],
            strides: [32; 16], topology: 4, strip: 0, blend: Some(vec![1, 2, 3]),
            depth: Some(vec![4, 5, 6]), raster: Some(vec![7, 8, 9]),
            colors: vec![Some(wgpu::TextureFormat::Rgba8Unorm), None],
            depth_format: Some(wgpu::TextureFormat::Depth24PlusStencil8), samples: 1,
            read_only_depth: false, read_only_stencil: false, masks: [[1, 2, 3, 4], [5, 6, 7, 8]] }
    }
    fn wait(store: &mut Store, writes: u64, errors: u64) {
        let start = Instant::now();
        loop {
            let status = store.status();
            if status["writes"].as_u64().unwrap() >= writes && status["errors"].as_u64().unwrap() >= errors { return; }
            assert!(start.elapsed() < Duration::from_secs(5), "worker timeout: {status}");
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn differences(base: &Recipe) -> Vec<Recipe> {
        let mut variants = Vec::new();
        macro_rules! change { ($r:ident . $($body:tt)*) => {{ let mut $r = base.clone(); $r.$($body)*; variants.push($r); }} }
        change!(r.vs.hash = "1123456789abcdef".into());
        change!(r.vs.pause = true);
        change!(r.vs.source_sha256 = "c".repeat(64));
        change!(r.ps = None);
        change!(r.ps.as_mut().unwrap().hash = "eedcba9876543210".into());
        change!(r.ps.as_mut().unwrap().pause = false);
        change!(r.ps.as_mut().unwrap().source_sha256 = "c".repeat(64));
        for field in 0..3 { change!(r.signature[0][field] += 1); }
        for field in 0..6 { change!(r.elements[0][field] += 1); }
        for slot in 0..16 { change!(r.strides[slot] += 1); }
        change!(r.topology = 5);
        change!(r.strip = 1);
        change!(r.strip = 2);
        change!(r.blend = None); change!(r.blend.as_mut().unwrap()[1] += 1);
        change!(r.depth = None); change!(r.depth.as_mut().unwrap()[1] += 1);
        change!(r.raster = None); change!(r.raster.as_mut().unwrap()[1] += 1);
        change!(r.colors[0] = Some(wgpu::TextureFormat::Bgra8Unorm));
        change!(r.colors[0] = None);
        change!(r.colors[1] = Some(wgpu::TextureFormat::Rgba8Unorm));
        change!(r.depth_format = None);
        change!(r.depth_format = Some(wgpu::TextureFormat::Depth32Float));
        change!(r.samples = 2);
        change!(r.read_only_depth = true);
        change!(r.read_only_stencil = true);
        for stage in 0..2 { for word in 0..4 { change!(r.masks[stage][word] += 1); } }
        variants
    }
    #[test]
    fn complete_schema_roundtrips_and_every_key_field_separates() {
        let base = recipe();
        let mut recipes = vec![base.clone()]; recipes.extend(differences(&base));
        assert!(recipes.iter().all(Recipe::valid));
        assert_eq!(recipes.iter().cloned().collect::<HashSet<_>>().len(), recipes.len());
        let bytes = encode("identity", recipes.clone()).unwrap();
        assert_eq!(decode("identity", &bytes).unwrap(), recipes);
        for variant in &recipes { assert!(serde_json::to_vec(variant).unwrap().len() < variant.size_bound()); }
    }
    #[test]
    fn schema_limits_reject_invalid_recipes() {
        macro_rules! invalid { ($r:ident . $($body:tt)*) => {{ let mut $r = recipe(); $r.$($body)*; assert!(!$r.valid(), "accepted {:?}", $r); }} }
        invalid!(r.vs.hash = "bad".into()); invalid!(r.vs.hash = "A".repeat(16));
        invalid!(r.vs.source_sha256 = "g".repeat(64)); invalid!(r.ps.as_mut().unwrap().source_sha256.clear());
        invalid!(r.signature = vec![[0; 3]; 33]); invalid!(r.signature[0][2] = 32);
        invalid!(r.elements = vec![[0; 6]; 33]); invalid!(r.elements[0][3] = 16); invalid!(r.elements[0][5] = 2);
        invalid!(r.topology = 0); invalid!(r.topology = 6); invalid!(r.strip = 3);
        invalid!(r.colors = vec![None; 9]); invalid!(r.samples = 0); invalid!(r.samples = 3); invalid!(r.samples = 128);
        invalid!(r.blend = Some(vec![0; 129])); invalid!(r.depth = Some(vec![0; 129])); invalid!(r.raster = Some(vec![0; 129]));
        let mut r = recipe(); r.strides = [0; 16]; assert!(r.valid());
    }
    #[test]
    fn corrupt_foreign_and_future_files_fail_closed() {
        let original = encode("identity", vec![recipe()]).unwrap();
        assert!(decode("other-build", &original).unwrap_err().contains("identity"));
        for key in ["version", "sha256", "recipes", "identity"] {
            let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
            match key {
                "version" => json[key] = 2.into(), "sha256" => json[key] = "0".repeat(64).into(),
                "recipes" => json[key][0]["strides"][0] = 123.into(), "identity" => json[key] = "foreign".into(), _ => unreachable!(),
            }
            assert!(decode("identity", &serde_json::to_vec(&json).unwrap()).is_err());
        }
        assert!(decode("identity", &original[..original.len()-1]).is_err());
        let mut json: serde_json::Value = serde_json::from_slice(&original).unwrap();
        json["recipes"][0]["unknown_pipeline_field"] = true.into();
        assert!(decode("identity", &serde_json::to_vec(&json).unwrap()).is_err());
        assert!(decode("identity", &encode("identity", vec![recipe(); LIMIT + 1]).unwrap()).is_err());
        assert!(decode("identity", &vec![b' '; FILE_LIMIT + 1]).is_err());
    }
    #[test]
    fn record_dedup_and_limits_bound_memory_without_discarding_prior_use() {
        let root = Root::new(); let mut store = root.store(false);
        for number in 0..LIMIT + 1 {
            let mut r = recipe(); r.vs.hash = format!("{number:016x}"); store.record(&r); store.record(&r);
        }
        assert_eq!(store.recipes.len(), LIMIT); assert_eq!(store.generation, LIMIT as u64);
        assert_eq!(store.limit_drops, 2); assert!(store.bytes_bound <= FILE_LIMIT);
        let mut invalid = recipe(); invalid.samples = 3; store.record(&invalid); assert_eq!(store.rejected, 1);
        // Worst-case valid records hit the byte budget before the entry limit.
        let mut large = root.store(false);
        for number in 0..LIMIT {
            let mut r = recipe(); r.vs.hash = format!("{number:016x}");
            r.signature = vec![[u32::MAX, u32::MAX, 31]; 32];
            r.elements = vec![[u32::MAX, u32::MAX, u32::MAX, 15, u32::MAX, 1]; 32];
            r.blend = Some(vec![u32::MAX; 128]); r.depth = r.blend.clone(); r.raster = r.blend.clone();
            large.record(&r);
        }
        assert!(large.recipes.len() < LIMIT); assert!(large.bytes_bound <= FILE_LIMIT);
        assert!(encode(&large.identity, large.recipes.iter().cloned().collect()).unwrap().len() <= FILE_LIMIT);
    }
    #[test]
    fn worker_persists_complete_atomic_json_and_preserves_loaded_records() {
        let root = Root::new(); let mut first = root.store(false);
        assert!(first.load().unwrap().is_empty()); assert!(!root.0.join(DIRECTORY).exists());
        first.record(&recipe()); first.checkpoint(); wait(&mut first, 1, 0);
        assert_eq!(first.status()["dirty"], false);
        assert_eq!(fs::metadata(root.0.join(DIRECTORY)).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(root.file()).unwrap().mode() & 0o777, 0o600);
        let mut second = root.store(false); assert_eq!(second.load().unwrap(), vec![recipe()]);
        second.checkpoint(); assert!(second.worker.is_none()); // Loading alone never rewrites.
        let mut different = recipe(); different.read_only_depth = true; second.record(&different);
        second.checkpoint();
        // During replacement a reader sees either entire last-good generation.
        while second.status()["inFlight"] == true {
            let bytes = fs::read(root.file()).unwrap(); let records = decode(&second.identity, &bytes).unwrap();
            assert!(records.len() == 1 || records.len() == 2);
        }
        wait(&mut second, 1, 0);
        let mut third = root.store(true); let records: HashSet<_> = third.load().unwrap().into_iter().collect();
        assert_eq!(records, [recipe(), different].into_iter().collect());
        assert_eq!(fs::read_dir(root.0.join(DIRECTORY)).unwrap().count(), 1);
    }
    #[test]
    fn read_only_never_creates_or_rewrites_files() {
        let root = Root::new(); let mut store = root.store(true);
        assert!(store.load().unwrap().is_empty()); store.record(&recipe()); store.checkpoint();
        assert!(store.worker.is_none()); assert!(!root.0.join(DIRECTORY).exists());
        root.install(&encode(&store.identity, vec![recipe()]).unwrap());
        let before = fs::read(root.file()).unwrap();
        let mut store = root.store(true); assert_eq!(store.load().unwrap().len(), 1);
        let mut different = recipe(); different.samples = 2; store.record(&different); store.checkpoint(); drop(store);
        assert_eq!(fs::read(root.file()).unwrap(), before);
    }
    #[test]
    fn application_group_writable_root_keeps_recipe_storage_private() {
        let root = Root::new();
        assert_eq!(fs::metadata(&root.0).unwrap().gid(), unsafe { libc::getegid() });
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o770)).unwrap();
        let mut store = root.store(false); store.record(&recipe()); store.checkpoint(); wait(&mut store, 1, 0);
        assert_eq!(root.store(true).load().unwrap(), vec![recipe()]);
        assert_eq!(fs::metadata(root.0.join(DIRECTORY)).unwrap().mode() & 0o777, 0o700);
        assert_eq!(fs::metadata(root.file()).unwrap().mode() & 0o777, 0o600);
        let before = fs::read(root.file()).unwrap();
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o772)).unwrap();
        let mut changed = recipe(); changed.samples = 2; store.record(&changed);
        store.last_capture = None; store.checkpoint(); wait(&mut store, 1, 1);
        assert_eq!(fs::read(root.file()).unwrap(), before);
    }
    #[test]
    fn capture_throttles_coalesces_and_retries_write_errors() {
        let root = Root::new(); let mut store = root.store(false);
        let external = Root::new(); symlink(&external.0, root.0.join(DIRECTORY)).unwrap();
        store.record(&recipe()); store.checkpoint(); wait(&mut store, 0, 1);
        assert_eq!(store.status()["dirty"], true); assert!(!root.file().exists());
        fs::remove_file(root.0.join(DIRECTORY)).unwrap();
        store.last_capture = None; store.checkpoint(); wait(&mut store, 1, 1);
        let mut different = recipe(); different.samples = 2; store.record(&different); store.checkpoint();
        assert_eq!(store.queued_generation, 1); assert_eq!(store.generation, 2);
        store.last_capture = None; store.checkpoint(); wait(&mut store, 2, 1);
        assert_eq!(decode(&store.identity, &fs::read(root.file()).unwrap()).unwrap().len(), 2);
    }
    #[test]
    fn private_storage_rejects_symlinks_permissions_hardlinks_and_oversize_files() {
        let root = Root::new(); let external = Root::new();
        let link = external.0.join("application-root"); symlink(&root.0, &link).unwrap();
        assert!(Store::new(&link, "identity".into(), true).load().is_err());
        assert_eq!(fs::metadata(&root.0).unwrap().gid(), unsafe { libc::getegid() });
        for mode in [0o770, 0o771] {
            fs::set_permissions(&root.0, fs::Permissions::from_mode(mode)).unwrap();
            assert!(root.store(true).load().unwrap().is_empty());
        }
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o772)).unwrap(); assert!(root.store(true).load().is_err());
        fs::set_permissions(&root.0, fs::Permissions::from_mode(0o750)).unwrap(); assert!(root.store(true).load().unwrap().is_empty());
        root.install(&encode("current-build-and-shaders", vec![recipe()]).unwrap());
        fs::set_permissions(root.0.join(DIRECTORY), fs::Permissions::from_mode(0o755)).unwrap(); assert!(root.store(true).load().is_err());
        fs::set_permissions(root.0.join(DIRECTORY), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(root.file(), fs::Permissions::from_mode(0o644)).unwrap(); assert!(root.store(true).load().is_err());
        fs::set_permissions(root.file(), fs::Permissions::from_mode(0o600)).unwrap();
        let hardlink = root.0.join("hardlink"); fs::hard_link(root.file(), &hardlink).unwrap(); assert!(root.store(true).load().is_err());
        fs::remove_file(hardlink).unwrap(); fs::remove_file(root.file()).unwrap();
        fs::write(external.0.join("target"), b"private").unwrap();
        symlink(external.0.join("target"), root.file()).unwrap(); assert!(root.store(true).load().is_err());
        fs::remove_file(root.file()).unwrap(); root.install(b"{}");
        OpenOptions::new().write(true).open(root.file()).unwrap().set_len(FILE_LIMIT as u64 + 1).unwrap();
        assert!(root.store(true).load().unwrap_err().contains("file_limit"));
    }
    #[test]
    fn teardown_worker_drains_final_coalesced_snapshot() {
        let root = Root::new(); let mut store = root.store(false);
        store.record(&recipe()); store.checkpoint(); wait(&mut store, 1, 0);
        let mut different = recipe(); different.samples = 2; store.record(&different); drop(store);
        let start = Instant::now();
        loop {
            let mut reader = root.store(true);
            if reader.load().unwrap().len() == 2 { break; }
            assert!(start.elapsed() < Duration::from_secs(5)); thread::sleep(Duration::from_millis(2));
        }
    }
}
'''


def main() -> None:
    root = Path(__file__).resolve().parents[1]
    source = root / "game-runtime/src/pso_warmup.rs"
    environment = os.environ.copy()
    cargo = shutil.which("cargo")
    if cargo is None:
        cargo_home = root / "runtime-probe/.toolchain/cargo"
        cargo = str(cargo_home / "bin/cargo")
        environment["CARGO_HOME"] = str(cargo_home)
        environment["RUSTUP_HOME"] = str(root / "runtime-probe/.toolchain/rustup")
        environment["PATH"] = str(cargo_home / "bin") + os.pathsep + environment["PATH"]
    if not Path(cargo).is_file():
        raise SystemExit("cargo unavailable: source game-runtime/scripts/env.sh first")
    with tempfile.TemporaryDirectory(prefix="pso-warmup-host-") as temporary:
        work = Path(temporary)
        (work / "src").mkdir()
        (work / "src/lib.rs").write_text(source.read_text() + "\n" + HARNESS)
        (work / "Cargo.toml").write_text('''[package]
name = "pso-warmup-host"
version = "0.0.0"
edition = "2021"
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "=0.10.9"
libc = "0.2"
wgpu = { package = "wgpu-types", version = "=30.0.1", default-features = false, features = ["std", "serde"] }
''')
        shutil.copyfile(root / "game-runtime/Cargo.lock", work / "Cargo.lock")
        subprocess.run([cargo, "test", "--offline", "--lib", "--manifest-path", str(work / "Cargo.toml"),
                        "--target-dir", str(Path(tempfile.gettempdir()) / "gta-pso-warmup-host-target"),
                        "--", "--nocapture"], cwd=work, env=environment, check=True)
    print(json.dumps({"status": "pass", "productionSource": str(source), "hostTests": 10,
                      "coverage": "complete schema, corruption, identity, bounded dedup, read-only, permissions, atomic worker persistence and retry",
                      "limits": "No GPU compilation, physical-device execution or performance measurement."}, ensure_ascii=False))


if __name__ == "__main__":
    main()
