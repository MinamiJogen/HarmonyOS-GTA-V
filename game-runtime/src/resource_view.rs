//! One immutable manifest/resource snapshot for both guest HTTPFS and native I/O.
//! Imported files and their package checksums remain intact on disk. Only this
//! process's frontend bytes and matching manifest length/version are derived.

#[path = "hud_layout.rs"]
mod hud_layout;

use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::sync::LazyLock;

const FRONTEND: &str = "common/data/ui/frontend.xml";

struct ResourceView {
    manifest: Option<Vec<u8>>,
    files: Vec<(String, u64)>,
    frontend: Option<Vec<u8>>,
}

static VIEW: LazyLock<ResourceView> = LazyLock::new(|| {
    let root = PathBuf::from(std::env::var("MUGUET_DATA").unwrap_or_default());
    let original = match std::fs::read(root.join("manifest.json")) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("[hud] resource manifest unavailable: {error}");
            return ResourceView { manifest: None, files: Vec::new(), frontend: None };
        }
    };
    match build_view(&original, || std::fs::read(root.join(FRONTEND)).map_err(|error| error.to_string())) {
        Ok(view) => view,
        Err(error) => {
            eprintln!("[hud] mobile layout skipped: {error}");
            // Even the fallback uses a shared manifest snapshot, so a runtime
            // never reports one length while its I/O thread reads another view.
            let files = parse_files(&original).unwrap_or_default();
            ResourceView { manifest: Some(original), files, frontend: None }
        }
    }
});

fn parse_files(bytes: &[u8]) -> Result<Vec<(String, u64)>, String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    value["files"].as_array().ok_or("manifest has no files")?.iter().map(|row| {
        Ok((row[0].as_str().ok_or("invalid manifest path")?.to_string(), row[1].as_u64().ok_or("invalid manifest length")?))
    }).collect()
}

fn build_view(original: &[u8], read_frontend: impl FnOnce() -> Result<Vec<u8>, String>) -> Result<ResourceView, String> {
    let mut value: serde_json::Value = serde_json::from_slice(original).map_err(|error| error.to_string())?;
    let rows = value["files"].as_array_mut().ok_or("manifest has no files")?;
    let indices: Vec<usize> = rows.iter().enumerate().filter_map(|(index, row)| (row[0].as_str() == Some(FRONTEND)).then_some(index)).collect();
    if indices.len() != 1 { return Err("manifest must contain one frontend.xml".into()); }
    let source = read_frontend()?;
    if rows[indices[0]][1].as_u64() != Some(source.len() as u64) { return Err("source frontend length differs from imported manifest".into()); }
    let frontend = hud_layout::derive(&source)?;
    rows[indices[0]][1] = serde_json::Value::from(frontend.len() as u64);
    let version = value["version"].as_str().unwrap_or("unknown");
    value["version"] = serde_json::Value::from(format!("{version}-mobile-hud-v1"));
    let manifest = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    let files = parse_files(&manifest)?;
    eprintln!("[hud] mobile frontend active: {} -> {} bytes; source-sha256={:x}, view-sha256={:x}, manifest-sha256={:x}", source.len(), frontend.len(), Sha256::digest(&source), Sha256::digest(&frontend), Sha256::digest(&manifest));
    Ok(ResourceView { manifest: Some(manifest), files, frontend: Some(frontend) })
}

pub fn manifest() -> Option<&'static [u8]> { VIEW.manifest.as_deref() }

pub fn files() -> &'static [(String, u64)] { &VIEW.files }

pub fn minimap_rect() -> Option<serde_json::Value> {
    let [x,y,width,height]=hud_layout::minimap_rect(VIEW.frontend.as_deref()?).ok()?;
    Some(serde_json::json!({"x":x,"y":y,"width":width,"height":height}))
}

/// An override is either the exact complete derived file, or absent. Slice at
/// the I/O boundary so arbitrary partial reads use the same length as HTTPFS.
pub fn overridden(path: &str) -> Option<&'static [u8]> {
    (path == FRONTEND).then(|| VIEW.frontend.as_deref()).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_matches_derived_bytes_without_changing_file_order_or_metadata() {
        let source = std::fs::read(std::env::var("MUGUET_TEST_FRONTEND").unwrap()).unwrap();
        let manifest = serde_json::json!({"version":"fixture", "mount":"/game/", "files":[["before",9,123],[FRONTEND,source.len(),456],["after",3,789]]});
        let original = serde_json::to_vec(&manifest).unwrap();
        let view = build_view(&original, || Ok(source)).unwrap();
        let derived = view.frontend.as_ref().unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(view.manifest.as_ref().unwrap()).unwrap();
        assert_eq!(view.files[1], (FRONTEND.into(), derived.len() as u64));
        assert_eq!(view.files[0], ("before".into(), 9));
        assert_eq!(view.files[2], ("after".into(), 3));
        assert_eq!(manifest["files"][1][2], 456);
        assert_eq!(manifest["version"], "fixture-mobile-hud-v1");
    }

    #[test]
    fn length_inconsistent_source_or_duplicate_manifest_is_rejected() {
        let source = std::fs::read(std::env::var("MUGUET_TEST_FRONTEND").unwrap()).unwrap();
        let manifest = serde_json::json!({"files":[[FRONTEND,source.len() + 1]]});
        assert!(build_view(&serde_json::to_vec(&manifest).unwrap(), || Ok(source)).is_err());
        let duplicate = serde_json::json!({"files":[[FRONTEND,1],[FRONTEND,1]]});
        assert!(build_view(&serde_json::to_vec(&duplicate).unwrap(), || Ok(Vec::new())).is_err());
    }
}
