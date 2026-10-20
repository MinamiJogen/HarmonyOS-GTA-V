#!/usr/bin/env python3
"""Host-test the actual process-local FG observation snapshot; no GPU build."""
from pathlib import Path
import shutil
import subprocess
import tempfile


def main():
    root = Path(__file__).resolve().parents[1]
    source = (root / "game-runtime/src/frame_interpolation.rs").read_text()
    begin = source.index("static LIVE_STATE:")
    end = source.index("fn note_live(", begin)
    production = source[begin:end]
    # Insert one host-only callback at an exact read boundary to deterministically
    # exercise a reset/new AI publication while the real snapshot is in flight.
    boundary = "    let after=LIVE_REVISION.load(Ordering::SeqCst);"
    assert production.count(boundary) == 1
    assert "loop" not in production
    for field in ("LIVE_AI_NS", "LIVE_GENERATION", "LIVE_STATE"):
        assert source.count(field + ".store(") == 1, "all writes must use the single publisher"
    assert "live_snapshot_current(snapshot,generation_before,crate::ffi::app_presentation_generation())" in source
    presented = source[source.index("        pub(crate) fn presented("):]
    assert 'if success&&display.kind=="ai" {\n                publish_live(2,' in presented
    assert "let fresh=current&&now.is_some_and" in source
    production = production.replace(boundary, "    test_read_interleave();\n" + boundary)
    harness = r'''
use std::sync::atomic::{AtomicU8,Ordering};
static INTERLEAVE:AtomicU8=AtomicU8::new(0);
fn test_read_interleave() {
    match INTERLEAVE.swap(0,Ordering::SeqCst) {
        1=>publish_live(1,0,8),
        2=>publish_live(2,200,7),
        _=>{}
    }
}
#[test]
fn stable_ai_is_a_coherent_current_generation_observation() {
    publish_live(2,100,7);
    let s=live_snapshot().unwrap();
    assert_eq!((s.state,s.last,s.generation),(2,100,7));
    assert!(live_snapshot_current(Some(s),7,7));
}
#[test]
fn reset_does_not_retain_the_old_ai_time() {
    publish_live(2,100,7);publish_live(1,0,8);
    let s=live_snapshot().unwrap();
    assert_eq!((s.state,s.last,s.generation),(1,0,8));
    assert!(!live_snapshot_current(Some(s),7,8));
}
#[test]
fn a_reader_never_spins_on_an_incomplete_publication() {
    publish_live(2,100,7);
    LIVE_REVISION.fetch_add(1,Ordering::SeqCst);
    LIVE_AI_NS.store(0,Ordering::SeqCst);
    LIVE_GENERATION.store(8,Ordering::SeqCst);
    assert!(live_snapshot().is_none());
    assert!(!live_snapshot_current(None,8,8));
    LIVE_STATE.store(1,Ordering::SeqCst);
    LIVE_REVISION.fetch_add(1,Ordering::SeqCst);
    assert_eq!(live_snapshot().unwrap().last,0);
}
#[test]
fn reset_between_the_data_read_and_final_revision_is_rejected() {
    publish_live(2,100,7);INTERLEAVE.store(1,Ordering::SeqCst);
    assert!(live_snapshot().is_none());
    let s=live_snapshot().unwrap();assert_eq!((s.state,s.last,s.generation),(1,0,8));
}
#[test]
fn same_generation_new_ai_between_reads_is_also_rejected() {
    publish_live(2,100,7);INTERLEAVE.store(2,Ordering::SeqCst);
    assert!(live_snapshot().is_none());
    let s=live_snapshot().unwrap();assert_eq!((s.state,s.last,s.generation),(2,200,7));
}
#[test]
fn app_generation_changes_and_stale_snapshots_cannot_be_current() {
    publish_live(2,100,7);let s=live_snapshot();
    assert!(!live_snapshot_current(s,7,8));
    assert!(!live_snapshot_current(s,8,8));
    publish_live(3,0,8);let s=live_snapshot().unwrap();
    assert_eq!((s.state,s.last),(3,0));assert!(live_snapshot_current(Some(s),8,8));
}
#[test]
fn concurrent_single_writer_and_reader_never_accept_mixed_fields() {
    publish_live(1,0,0);
    let writer=std::thread::spawn(||for generation in 1..=10000 {
        publish_live(2,generation*10,generation);
        publish_live(1,0,generation+1);
    });
    for _ in 0..10000 {
        if let Some(s)=live_snapshot() {
            match s.state {2=>assert_eq!(s.last,s.generation*10),1=>assert_eq!(s.last,0),_=>panic!("unexpected state")}
        }
    }
    writer.join().unwrap();assert_eq!(live_snapshot().unwrap().state,1);
}
'''
    rustc = shutil.which("rustc")
    if rustc is None:
        raise SystemExit("rustc unavailable: source game-runtime/scripts/env.sh first")
    with tempfile.TemporaryDirectory(prefix="frame-generation-live-") as directory:
        path = Path(directory) / "live.rs"
        binary = Path(directory) / "live-test"
        path.write_text(production + harness)
        subprocess.run([rustc, "--edition=2021", "--test", str(path), "-o", str(binary)], check=True)
        subprocess.run([str(binary), "--test-threads=1"], check=True)
    print("actual production snapshot + forced reset/AI interleavings passed; no rendering or device claim")


if __name__ == "__main__":
    main()
