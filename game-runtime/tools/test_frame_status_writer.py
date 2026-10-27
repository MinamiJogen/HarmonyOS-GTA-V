#!/usr/bin/env python3
"""Exercise the production status writer in a small offline host-only crate."""
import argparse
import os
from pathlib import Path
import re
import subprocess
import tempfile

TESTS = r'''
#[cfg(test)] mod tests {
    use super::*;
    use std::{sync::mpsc,thread,time::{Duration,Instant}};
    static IDS:AtomicU64=AtomicU64::new(0);
    fn directory()->PathBuf {
        let p=std::env::temp_dir().join(format!("muguet-status-contract-{}-{}",std::process::id(),IDS.fetch_add(1,Ordering::Relaxed)));
        std::fs::create_dir(&p).unwrap();p
    }
    fn wait(condition:impl Fn()->bool) {
        let deadline=Instant::now()+Duration::from_secs(3);
        while !condition(){assert!(Instant::now()<deadline,"worker progress timeout");thread::sleep(Duration::from_millis(1));}
    }
    fn value(n:u64)->Value {json!({"sourcePresents":n,"aiPresents":n,"repeatPresents":2,"actualTotal":n*2+2,
        "recentPresents":(0..180).map(|slot|json!({"slot":slot,"kind":"ai","sourceSequence":slot/2})).collect::<Vec<_>>(),
        "outputCaptureDirectory":"fixed-capture-identity","outputCaptureFrames":[{"slot":44,"kind":"ai","sourceSequence":22}],
        "timestampClock":"CLOCK_MONOTONIC_ns_explicit_clock_gettime; Instant_elapsed_is_separate"})}
    fn read(root:&Path)->Value {serde_json::from_slice(&std::fs::read(root.join("frame-interpolation-status.json")).unwrap()).unwrap()}
    #[test] fn slow_sink_is_bounded_nonblocking_and_drop_drains() {
        let root=directory();let target=root.clone();let(entered_tx,entered_rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();let mut first=true;
        let writer=Arc::new(Writer::with_sink(move|s,stats|{if first{first=false;entered_tx.send(()).unwrap();release_rx.recv().unwrap();}publish(&target,s,stats,0o600)}));
        let stats=writer.stats.clone();writer.submit(value(1),super::super::monotonic_ns(),super::super::monotonic_ns());entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let producer=writer.clone();let(done_tx,done_rx)=mpsc::channel();
        let producer=thread::spawn(move||{producer.submit(value(2),None,None);producer.submit(value(3),None,None);done_tx.send(()).unwrap();});
        let done=done_rx.recv_timeout(Duration::from_secs(1));
        if done.is_err(){release_tx.send(()).unwrap();panic!("GPU submit waited for slow sink");}
        producer.join().unwrap();assert_eq!(stats.full_drops.load(Ordering::Relaxed),1);
        drop(writer);assert!(!stats.exited.load(Ordering::Acquire));release_tx.send(()).unwrap();wait(||stats.exited.load(Ordering::Acquire));
        assert_eq!(stats.published.load(Ordering::Relaxed),2);assert_eq!(read(&root)["sourcePresents"],2);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test] fn snapshot_drop_does_not_reset_actual_frame_counters() {
        let root=directory();let target=root.clone();let(entered_tx,entered_rx)=mpsc::channel();let(release_tx,release_rx)=mpsc::channel();let mut first=true;
        let writer=Writer::with_sink(move|s,stats|{if first{first=false;entered_tx.send(()).unwrap();release_rx.recv().unwrap();}publish(&target,s,stats,0o600)});
        let stats=writer.stats.clone();writer.submit(value(10),None,None);entered_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        writer.submit(value(11),None,None);writer.submit(value(12),None,None);release_tx.send(()).unwrap();wait(||stats.last_sequence.load(Ordering::Relaxed)==2);
        let original=value(13);writer.submit(original.clone(),Some(100),Some(120));wait(||stats.last_sequence.load(Ordering::Relaxed)==4);
        let actual=read(&root);for key in ["sourcePresents","aiPresents","repeatPresents","actualTotal","recentPresents","outputCaptureDirectory","outputCaptureFrames","timestampClock"]{assert_eq!(actual[key],original[key]);}
        assert_eq!(actual["statusWriter"]["queueFullSnapshotDrops"],1);assert_eq!(actual["statusSnapshotGenerationNs"],20);
        assert_eq!(actual["statusWriter"]["lastCompletedSnapshotSequence"],2);assert!(actual["statusWriter"]["lastPublishCompleteMonotonicNs"].as_u64().unwrap()>0);
        drop(writer);wait(||stats.exited.load(Ordering::Acquire));std::fs::remove_dir_all(root).unwrap();
    }
    #[test] fn atomic_snapshots_permissions_and_reader_consistency() {
      for mode in [0o600,0o660] {
        let root=directory();let target=root.clone();let writer=Writer::with_sink(move|s,stats|publish(&target,s,stats,mode));let stats=writer.stats.clone();writer.submit(value(0),None,None);wait(||stats.last_sequence.load(Ordering::Relaxed)==1);
        let stop=Arc::new(AtomicBool::new(false));let reader_stop=stop.clone();let reader_root=root.clone();
        let reader=thread::spawn(move||{let mut reads=0;while !reader_stop.load(Ordering::Acquire){let v=read(&reader_root);let n=v["sourcePresents"].as_u64().unwrap();assert_eq!(v["aiPresents"],n);assert_eq!(v["actualTotal"],n*2+2);assert_eq!(v["recentPresents"].as_array().unwrap().len(),180);reads+=1;}reads});
        for n in 1..40 {writer.submit(value(n),None,None);wait(||stats.last_sequence.load(Ordering::Relaxed)==n+1);}
        stop.store(true,Ordering::Release);assert!(reader.join().unwrap()>0);
        assert_eq!(std::fs::metadata(root.join("frame-interpolation-status.json")).unwrap().permissions().mode()&0o777,mode);
        assert!(!root.join(format!(".frame-interpolation-status-{}.json.tmp",std::process::id())).exists());
        drop(writer);wait(||stats.exited.load(Ordering::Acquire));std::fs::remove_dir_all(root).unwrap();
      }
    }
    #[test] fn monitor_flag_accepts_only_exact_one_including_os_bytes() {
        use std::os::unix::ffi::OsStringExt;
        assert_eq!(mode_for_monitor_flag(None),0o600);
        for flag in ["","0","true","yes","01","1 "," 1","1\n","１"] {
            assert_eq!(mode_for_monitor_flag(Some(std::ffi::OsStr::new(flag))),0o600);
        }
        let invalid=std::ffi::OsString::from_vec(vec![b'1',0xff]);
        assert_eq!(mode_for_monitor_flag(Some(&invalid)),0o600);
        assert_eq!(mode_for_monitor_flag(Some(std::ffi::OsStr::new("1"))),0o660);
    }
    #[test] fn stale_temp_permissions_are_corrected_only_for_owned_snapshot() {
      for mode in [0o600,0o660] {
        let root=directory();let other=root.join("model.json");std::fs::write(&other,b"unchanged").unwrap();
        std::fs::set_permissions(&other,std::fs::Permissions::from_mode(0o600)).unwrap();
        let tmp=root.join(format!(".frame-interpolation-status-{}.json.tmp",std::process::id()));
        std::fs::write(&tmp,b"stale").unwrap();std::fs::set_permissions(&tmp,std::fs::Permissions::from_mode(0o777)).unwrap();
        let stats=Stats::default();publish(&root,Snapshot{value:value(17),sequence:1,began_ns:None,generated_ns:None},&stats,mode);
        assert_eq!(stats.published.load(Ordering::Relaxed),1);assert_eq!(read(&root)["sourcePresents"],17);
        assert_eq!(std::fs::metadata(root.join("frame-interpolation-status.json")).unwrap().permissions().mode()&0o777,mode);
        assert_eq!(std::fs::metadata(&other).unwrap().permissions().mode()&0o777,0o600);assert_eq!(std::fs::read(other).unwrap(),b"unchanged");
        assert!(!tmp.exists());std::fs::remove_dir_all(root).unwrap();
      }
    }
    #[test] #[ignore] fn startup_mode_is_frozen_and_atomic_result_matches() {
        let expected=u32::from_str_radix(&std::env::var("MUGUET_WRITER_EXPECTED_MODE").unwrap(),8).unwrap();
        let root=directory();let writer=Writer::new(root.clone());let stats=writer.stats.clone();writer.submit(value(11),None,None);
        wait(||stats.last_sequence.load(Ordering::Relaxed)==1);assert_eq!(read(&root)["sourcePresents"],11);
        assert_eq!(std::fs::metadata(root.join("frame-interpolation-status.json")).unwrap().permissions().mode()&0o777,expected);
        std::env::set_var("MUGUET_NATIVE_BENCHMARK_MONITOR",if expected==0o600{"1"}else{"0"});
        let second_root=directory();let second=Writer::new(second_root.clone());let second_stats=second.stats.clone();second.submit(value(12),None,None);
        wait(||second_stats.last_sequence.load(Ordering::Relaxed)==1);assert_eq!(read(&second_root)["sourcePresents"],12);
        assert_eq!(std::fs::metadata(second_root.join("frame-interpolation-status.json")).unwrap().permissions().mode()&0o777,expected);
        assert!(!root.join(format!(".frame-interpolation-status-{}.json.tmp",std::process::id())).exists());
        assert!(!second_root.join(format!(".frame-interpolation-status-{}.json.tmp",std::process::id())).exists());
        drop(writer);drop(second);wait(||stats.exited.load(Ordering::Acquire)&&second_stats.exited.load(Ordering::Acquire));
        std::fs::remove_dir_all(root).unwrap();std::fs::remove_dir_all(second_root).unwrap();
    }
    #[test] fn io_failure_is_observable_and_recovers_without_partial_publish() {
        let root=directory();let final_path=root.join("frame-interpolation-status.json");std::fs::create_dir(&final_path).unwrap();
        let writer=Writer::new(root.clone());let stats=writer.stats.clone();writer.submit(value(1),None,None);wait(||stats.io_errors.load(Ordering::Relaxed)==1);
        assert_eq!(stats.published.load(Ordering::Relaxed),0);std::fs::remove_dir(&final_path).unwrap();writer.submit(value(2),None,None);wait(||stats.last_sequence.load(Ordering::Relaxed)==2);
        assert_eq!(read(&root)["statusWriter"]["fileWriteErrors"],1);assert_eq!(read(&root)["sourcePresents"],2);
        drop(writer);wait(||stats.exited.load(Ordering::Acquire));std::fs::remove_dir_all(root).unwrap();
    }
    #[test] fn nofollow_oversize_serialization_and_disconnection_are_explicit() {
        let root=directory();let victim=root.join("victim");std::fs::write(&victim,b"preserve").unwrap();
        let tmp=root.join(format!(".frame-interpolation-status-{}.json.tmp",std::process::id()));std::os::unix::fs::symlink(&victim,&tmp).unwrap();
        let stats=Arc::new(Stats::default());publish(&root,Snapshot{value:value(1),sequence:1,began_ns:None,generated_ns:None},&stats,0o600);
        assert_eq!(stats.io_errors.load(Ordering::Relaxed),1);assert_eq!(std::fs::read(&victim).unwrap(),b"preserve");std::fs::remove_file(tmp).unwrap();
        publish(&root,Snapshot{value:json!({"body":"x".repeat(MAX_BYTES)}),sequence:2,began_ns:None,generated_ns:None},&stats,0o600);assert_eq!(stats.oversize_drops.load(Ordering::Relaxed),1);
        publish(&root,Snapshot{value:json!([]),sequence:3,began_ns:None,generated_ns:None},&stats,0o600);assert_eq!(stats.serialize_errors.load(Ordering::Relaxed),1);
        assert!(!root.join("frame-interpolation-status.json").exists());
        let(sender,receiver)=mpsc::sync_channel(1);drop(receiver);let writer=Writer{sender:Some(sender),stats:stats.clone()};writer.submit(value(4),None,None);
        assert_eq!(stats.disconnected_drops.load(Ordering::Relaxed),1);assert_eq!(stats.last_error.load(Ordering::Relaxed),5);
        std::fs::remove_dir_all(root).unwrap();
    }
}
'''


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--work-dir', type=Path)
    args = parser.parse_args()
    runtime = Path(__file__).resolve().parents[1]
    source = (runtime / 'src/frame_interpolation.rs').read_text()
    clock = source[source.index('pub(crate) fn monotonic_ns()'):source.index('/// Diagnostic only:')]
    begin = source.index('mod status_writer {')
    module = source[begin:source.index('// A normal runtime has no references', begin)].rstrip()
    assert module.endswith('}')
    output = args.work_dir or Path(tempfile.mkdtemp(prefix='muguet-status-writer-'))
    (output / 'src').mkdir(parents=True, exist_ok=True)
    lock = (runtime / 'Cargo.lock').read_text()
    version = lambda name: re.search(r'\[\[package\]\]\nname = "' + name + r'"\nversion = "([^"]+)"', lock).group(1)
    (output / 'Cargo.toml').write_text('[package]\nname="frame-status-writer-contract"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nserde_json="=' + version('serde_json') + '"\nlibc="=' + version('libc') + '"\n')
    (output / 'src/lib.rs').write_text('#![allow(dead_code)]\n' + clock + module[:-1] + TESTS + '\n}\n')
    print(f'Host-only contracts: {output}', flush=True)
    command = ['cargo', 'test', '--offline', '--manifest-path', str(output / 'Cargo.toml')]
    host_env = os.environ.copy()
    host_env.pop('MUGUET_NATIVE_BENCHMARK_MONITOR', None)
    subprocess.run(command, check=True, env=host_env)
    for flag in [None, '1', '0', 'true', '01', '1 ', ' 1', '1\n']:
        case_env = host_env.copy()
        if flag is not None:
            case_env['MUGUET_NATIVE_BENCHMARK_MONITOR'] = flag
        case_env['MUGUET_WRITER_EXPECTED_MODE'] = '660' if flag == '1' else '600'
        print(f'Isolated startup flag={flag!r} expected={case_env["MUGUET_WRITER_EXPECTED_MODE"]}', flush=True)
        subprocess.run(command + ['--lib', 'status_writer::tests::startup_mode_is_frozen_and_atomic_result_matches',
                                  '--', '--ignored', '--exact', '--test-threads=1'], check=True, env=case_env)


if __name__ == '__main__':
    main()
