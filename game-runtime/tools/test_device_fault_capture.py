#!/usr/bin/env python3
"""Run the production bounded collector against an ash ABI recording mock."""
from pathlib import Path
import hashlib
import json
import os
import subprocess

root = Path(__file__).resolve().parents[1]
source_path = root / 'vendor/wgpu-hal/src/vulkan/device_fault.rs'
source = source_path.read_text()
# Retain the full Diagnostics and collect implementation verbatim. The final
# impl only wires DeviceShared to this module and needs the rest of the HAL.
actual = source.split('\nimpl super::DeviceShared {', 1)[0]
out = root / 'logs/device-fault-collector-model'
(out / 'src').mkdir(parents=True, exist_ok=True)
registry = root.parent / 'runtime-probe/.toolchain/cargo/registry/src/index.crates.io-1949cf8c6b5b557f'
ash = next(registry.glob('ash-0.38.0+1.3.281'))
log = next(registry.glob('log-0.4.34'))
(out / 'Cargo.toml').write_text(f'''[package]
name = "muguet-device-fault-collector-model"
version = "0.0.0"
edition = "2021"
[workspace]
[dependencies]
ash = {{ path = {json.dumps(str(ash))}, default-features = false, features = ["std", "debug"] }}
log = {{ path = {json.dumps(str(log))} }}
''')
(out / 'src/actual_device_fault.rs').write_text(actual)
tests = r'''#![allow(dead_code)]
extern crate alloc;
mod actual_device_fault;
#[cfg(test)] mod tests {
 use super::actual_device_fault::*;
 use ash::vk;
 use std::{path::PathBuf, sync::atomic::{AtomicUsize,Ordering}};
 static NEXT:AtomicUsize=AtomicUsize::new(0);
 fn dir()->PathBuf {
  let p=std::env::temp_dir().join(format!("mg-fault-collector-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));
  std::fs::create_dir_all(&p).unwrap();p
 }
 fn report(p:&std::path::Path)->String {std::fs::read_to_string(p.join("device-fault.txt")).unwrap()}
 fn counts_header(c:&vk::DeviceFaultCountsEXT<'_>){
  assert_eq!(c.s_type,vk::StructureType::DEVICE_FAULT_COUNTS_EXT);assert!(c.p_next.is_null());
 }
 #[test] fn ash_layout_matches_64_bit_vulkan_abi(){
  assert_eq!(std::mem::size_of::<usize>(),8);
  assert_eq!(std::mem::size_of::<vk::DeviceFaultCountsEXT>(),32);
  assert_eq!(std::mem::size_of::<vk::DeviceFaultInfoEXT>(),296);
  assert_eq!(std::mem::size_of::<vk::DeviceFaultAddressInfoEXT>(),24);
  assert_eq!(std::mem::size_of::<vk::DeviceFaultVendorInfoEXT>(),272);
 }
 #[test] fn zero_counts_use_null_arrays_and_valid_struct_headers(){
  let p=dir();let mut calls=0;
  collect(&p,"zero",true,|c,i|{
   calls+=1;counts_header(c);
   assert_eq!((c.address_info_count,c.vendor_info_count,c.vendor_binary_size),(0,0,0));
   if calls==1 {assert!(i.is_null())}else{unsafe{
    let i=&*i;assert_eq!(i.s_type,vk::StructureType::DEVICE_FAULT_INFO_EXT);assert!(i.p_next.is_null());
    assert!(i.p_address_infos.is_null()&&i.p_vendor_infos.is_null()&&i.p_vendor_binary_data.is_null());
   }}vk::Result::SUCCESS
  }).unwrap();
  assert_eq!(calls,2);assert!(report(&p).contains("complete=true"));assert!(!p.join("vendor-binary.bin").exists());
 }
 #[test] fn huge_reported_counts_are_capped_and_incomplete_is_persisted(){
  let p=dir();let mut calls=0;
  collect(&p,"capped",true,|c,i|{
   calls+=1;counts_header(c);
   if i.is_null(){c.address_info_count=u32::MAX;c.vendor_info_count=u32::MAX;c.vendor_binary_size=u64::MAX;return vk::Result::SUCCESS}
   assert_eq!((c.address_info_count,c.vendor_info_count,c.vendor_binary_size),(MAX_ADDRESSES,MAX_VENDOR_INFOS,MAX_VENDOR_BINARY));
   unsafe{
    let i=&mut *i;assert!(!i.p_address_infos.is_null()&&!i.p_vendor_infos.is_null()&&!i.p_vendor_binary_data.is_null());
    (*i.p_address_infos.add(MAX_ADDRESSES as usize-1)).reported_address=0x1234;
    (*i.p_vendor_infos.add(MAX_VENDOR_INFOS as usize-1)).vendor_fault_code=0x5678;
    *(i.p_vendor_binary_data as *mut u8).add(MAX_VENDOR_BINARY as usize-1)=0xab;
   }vk::Result::INCOMPLETE
  }).unwrap();
  let t=report(&p);assert_eq!(calls,2);assert!(t.contains("infoResult=INCOMPLETE")&&t.contains("complete=false")&&t.contains("capacityClipped=true"));
  let bin=std::fs::read(p.join("vendor-binary.bin")).unwrap();assert_eq!(bin.len(),MAX_VENDOR_BINARY as usize);assert_eq!(bin[bin.len()-1],0xab);
  assert_eq!(t.lines().filter(|l|l.starts_with("address[")).count(),MAX_ADDRESSES as usize);
 }
 #[test] fn oversized_returned_counts_never_expand_the_initialized_output(){
  let p=dir();
  collect(&p,"bad returned count",true,|c,i|{
   if i.is_null(){c.address_info_count=2;c.vendor_info_count=1;c.vendor_binary_size=2;return vk::Result::SUCCESS}
   unsafe{let i=&mut *i;(*i.p_address_infos.add(1)).reported_address=0x99;*(i.p_vendor_binary_data as *mut u8)=7;}
   c.address_info_count=u32::MAX;c.vendor_info_count=u32::MAX;c.vendor_binary_size=u64::MAX;vk::Result::SUCCESS
  }).unwrap();
  let t=report(&p);assert!(t.contains("driverReturnedOutOfCapacityCount=true")&&t.contains("complete=false"));
  assert_eq!(t.lines().filter(|l|l.starts_with("address[")).count(),2);
  assert_eq!(t.lines().filter(|l|l.starts_with("vendor[")).count(),1);
  assert_eq!(std::fs::read(p.join("vendor-binary.bin")).unwrap(),[7,0]);
 }
 #[test] fn count_query_failure_is_synced_and_never_issues_a_fill(){
  let p=dir();let mut calls=0;
  collect(&p,"counts fail",true,|_,i|{calls+=1;assert!(i.is_null());vk::Result::ERROR_OUT_OF_HOST_MEMORY}).unwrap();
  assert_eq!(calls,1);let t=report(&p);assert!(t.contains("countsResult=ERROR_OUT_OF_HOST_MEMORY"));assert!(!t.contains("phase=before-info"));
 }
 #[test] fn fill_failure_keeps_phase_but_does_not_parse_unsuccessful_outputs(){
  let p=dir();let mut calls=0;
  collect(&p,"fill fail",true,|c,i|{
   calls+=1;if i.is_null(){c.address_info_count=1;c.vendor_binary_size=8;vk::Result::SUCCESS}
   else{c.address_info_count=u32::MAX;c.vendor_binary_size=u64::MAX;vk::Result::ERROR_UNKNOWN}
  }).unwrap();
  let t=report(&p);assert_eq!(calls,2);assert!(t.contains("phase=after-info")&&t.contains("infoResult=ERROR_UNKNOWN"));
  assert!(!t.contains("description=")&&!p.join("vendor-binary.bin").exists());
 }
 #[test] fn unsupported_vendor_binary_is_never_given_memory_or_capacity(){
  let p=dir();
  collect(&p,"binary off",false,|c,i|{
   if i.is_null(){c.vendor_binary_size=100;return vk::Result::SUCCESS}
   assert_eq!(c.vendor_binary_size,0);unsafe{assert!((*i).p_vendor_binary_data.is_null());}vk::Result::INCOMPLETE
  }).unwrap();
  assert!(!p.join("vendor-binary.bin").exists());assert!(report(&p).contains("vendorBinaryCapacity=0"));
 }
 #[test] fn descriptions_without_nul_and_invalid_utf8_are_bounded(){
  let p=dir();
  collect(&p,"bounded text",false,|c,i|{
   if i.is_null(){c.vendor_info_count=1;return vk::Result::SUCCESS}
   unsafe{(*i).description.fill(0xffu8 as core::ffi::c_char);(*(*i).p_vendor_infos).description.fill(b'X' as core::ffi::c_char);}
   vk::Result::SUCCESS
  }).unwrap();
  let t=report(&p);assert_eq!(t.matches('\u{fffd}').count(),256);assert!(t.contains(&"X".repeat(256)));assert!(t.len()<2500);
 }
}
'''
(out / 'src/lib.rs').write_text(tests)
env = dict(os.environ)
env['CARGO_HOME'] = str(root.parent / 'runtime-probe/.toolchain/cargo')
env['RUSTUP_HOME'] = str(root.parent / 'runtime-probe/.toolchain/rustup')
env['PATH'] = env['CARGO_HOME'] + '/bin:' + env.get('PATH', '')
result = subprocess.run(['cargo', 'test', '--offline', '--manifest-path', str(out/'Cargo.toml')], env=env, text=True, capture_output=True)
(root/'logs/device-fault-collector-tests.log').write_text(result.stdout+result.stderr)
report = {'method': 'Full production Diagnostics and collect implementation extracted verbatim; only DeviceShared routing impl omitted. Real cached ash0.38 structs/function ABI, host mock query, actual bounded file writes. No GPU/device fault induced.',
          'source': str(source_path.relative_to(root)), 'sourceSHA256': hashlib.sha256(source.encode()).hexdigest(),
          'testExitCode': result.returncode, 'testLog':'logs/device-fault-collector-tests.log', 'tests':8}
(out/'report.json').write_text(json.dumps(report,indent=2)+'\n')
print(result.stdout, result.stderr)
raise SystemExit(result.returncode)
