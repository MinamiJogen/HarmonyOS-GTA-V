#!/usr/bin/env python3
"""Run the actual Vulkan cached readback policy without creating a GPU device."""
from pathlib import Path
import subprocess
import tempfile

TESTS = r'''
#[cfg(test)] mod tests {
    use super::cached_readback_policy::{plan, atom_range, fallback_type};
    use ash::vk;
    fn ty(flags: vk::MemoryPropertyFlags) -> vk::MemoryType { vk::MemoryType { property_flags:flags,heap_index:0 } }
    fn request() -> vk::MemoryRequirements { vk::MemoryRequirements {size:257,alignment:64,memory_type_bits:0b1111} }
    fn cached() -> vk::MemoryPropertyFlags { vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_CACHED }
    #[test] fn noncoherent_cached_type_is_restricted_and_allocation_is_atom_isolated() {
        let types=[ty(vk::MemoryPropertyFlags::HOST_VISIBLE|vk::MemoryPropertyFlags::HOST_COHERENT),ty(cached()),ty(vk::MemoryPropertyFlags::DEVICE_LOCAL)];
        let planned=plan(request(),u32::MAX,&types,256).unwrap();
        assert_eq!(planned.memory_type_bits,0b10);assert_eq!(planned.size,512);assert_eq!(planned.alignment,256);
        let mut req=request();req.alignment=4096;assert_eq!(plan(req,u32::MAX,&types,256).unwrap().alignment,4096);
    }
    #[test] fn coherent_cached_retains_existing_allocator_preference() {
        let types=[ty(cached()),ty(cached()|vk::MemoryPropertyFlags::HOST_COHERENT)];
        assert!(plan(request(),u32::MAX,&types,256).is_none());
        // A coherent type excluded by Vulkan requirements cannot suppress an eligible cached type.
        let mut req=request();req.memory_type_bits=1;
        assert_eq!(plan(req,u32::MAX,&types,256).unwrap().memory_type_bits,1);
    }
    #[test] fn memory_type_masks_and_forbidden_flags_are_never_bypassed() {
        let types=[ty(cached()),ty(cached()|vk::MemoryPropertyFlags::PROTECTED),ty(cached()|vk::MemoryPropertyFlags::LAZILY_ALLOCATED),ty(vk::MemoryPropertyFlags::HOST_CACHED)];
        assert_eq!(plan(request(),u32::MAX,&types,256).unwrap().memory_type_bits,1);
        assert!(plan(request(),0b1110,&types,256).is_none());
        let mut req=request();req.memory_type_bits=0b1110;assert!(plan(req,u32::MAX,&types,256).is_none());
        req.memory_type_bits=0;assert!(plan(req,u32::MAX,&types,256).is_none());
    }
    #[test] fn padding_overflow_and_invalid_alignment_fall_back() {
        let types=[ty(cached())];
        for atom in [0,3,255] { assert!(plan(request(),u32::MAX,&types,atom).is_none()); }
        for alignment in [0,3] { let mut req=request();req.alignment=alignment;assert!(plan(req,u32::MAX,&types,256).is_none()); }
        for size in [0,u64::MAX] { let mut req=request();req.size=size;assert!(plan(req,u32::MAX,&types,256).is_none()); }
        assert!(atom_range(u64::MAX,8,16,255).is_none());
        assert!(atom_range(0,8,4,255).is_none());
    }
    #[test] fn original_allocator_readback_type_is_reported_with_its_exact_preference() {
        let types=[ty(cached()),ty(vk::MemoryPropertyFlags::HOST_VISIBLE|vk::MemoryPropertyFlags::HOST_COHERENT),ty(cached()|vk::MemoryPropertyFlags::HOST_COHERENT)];
        assert_eq!(fallback_type(0b111,&types),Some(2));
        assert_eq!(fallback_type(0b011,&types),Some(1));
        assert_eq!(fallback_type(0b001,&types),None);
    }
    #[test] fn invalidation_covers_unaligned_tail_without_touching_neighbor_allocations() {
        assert_eq!(atom_range(512,128,384,255),Some((512,512)));
        assert_eq!(atom_range(512,256,512,255),Some((768,256)));
        for atom in [1,64,256,4096] {
            for size in [4,12,256,260,1020] {
                let planned=plan(vk::MemoryRequirements{size,alignment:64,memory_type_bits:1},1,&[ty(cached())],atom).unwrap();
                let offset=atom*7;
                for start in (0..size).step_by(8) {
                    for end in ((start+4)..=size).step_by(4) {
                        let (at,len)=atom_range(offset,start,end,atom-1).unwrap();
                        assert!(at<=offset+start && at+len>=offset+end,"invalidated range missed requested bytes");
                        assert!(at>=offset && at+len<=offset+planned.size,"invalidated a neighbor allocation");
                        assert_eq!(at%atom,0);assert_eq!(len%atom,0);
                    }
                }
            }
        }
    }
    #[test] fn empty_ranges_do_not_invalidate_zero_bytes_or_expand_to_a_neighboring_atom() {
        for at in [0,128,256,512] { assert!(atom_range(512,at,at,255).is_none()); }
    }
}
'''


def main():
    runtime = Path(__file__).resolve().parents[1]
    device = (runtime / 'vendor/wgpu-hal/src/vulkan/device.rs').read_text()
    begin = device.index('mod cached_readback_policy {')
    end = device.index('\n#[cfg(target_env = "ohos")]\nfn cached_readback_enabled()', begin)
    policy = device[begin:end]
    with tempfile.TemporaryDirectory(prefix='cached-readback-policy-') as directory:
        root = Path(directory)
        (root / 'src').mkdir()
        (root / 'Cargo.toml').write_text('[package]\nname="cached-readback-policy-contract"\nversion="0.0.0"\nedition="2021"\n[workspace]\n[dependencies]\nash={version="=0.38.0",default-features=false,features=["debug"]}\n')
        (root / 'src/lib.rs').write_text('#![allow(dead_code)]\n' + policy + TESTS)
        subprocess.run(['cargo', 'test', '--offline', '--manifest-path', str(root / 'Cargo.toml')], check=True)
    assert 'if is_cpu_read && !is_cpu_write' in device
    assert 'value == "1"' in device
    assert 'MemoryLocation::Unknown, &planned' in device
    assert 'name, requirements: planned, location: gpu_allocator::MemoryLocation::Unknown' in device
    assert 'self.error_if_would_oom_on_resource_allocation(location, &requirements)' in device
    assert 'ownership: super::BufferOwnership::Managed' in device
    assert 'Some(ranges.filter_map(move |range| {' in device
    assert 'if range.is_empty() { return None; }' in device
    print('Cached readback policy contracts passed; allocation success, device coherence and performance require device validation.')


if __name__ == '__main__':
    main()
