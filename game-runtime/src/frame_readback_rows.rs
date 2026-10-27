//! Copy the existing guarded mapped RGBA layout into owned model/debug input.
//! Geometry, guard/error order and the RGB alpha-stripping path match the
//! original reader; raw NPU input copies each active row in one operation.
pub(super) fn copy_guarded_rgba(view:&[u8],width:u32,height:u32,pitch:usize,
    bytes:usize,guard:usize,sentinel:u8,raw:bool)->Result<Vec<u8>,String> {
    if view.len()!=bytes||!view[..guard].iter().chain(&view[bytes-guard..]).all(|&v|v==sentinel) {
        return Err("readback length or prefix/suffix guards changed".into());
    }
    let mut rgb=Vec::with_capacity(width as usize*height as usize*if raw{4}else{3});
    for row in 0..height as usize {
        let at=guard+row*pitch;
        let active=&view[at..at+width as usize*4];
        if raw {rgb.extend_from_slice(active);}
        else {for rgba in active.chunks_exact(4) {rgb.extend_from_slice(&rgba[..3]);}}
        if !view[at+width as usize*4..at+pitch].iter().all(|&v|v==sentinel) {
            return Err("readback row padding guard changed".into());
        }
    }
    Ok(rgb)
}

#[cfg(test)]
mod tests {
    use super::*;
    const GUARD:usize=64;
    const SENTINEL:u8=0xa7;

    // Frozen original pixel reader; compare byte results, exact errors and
    // existing slice-bound panics rather than inventing a new layout policy.
    fn original(view:&[u8],width:u32,height:u32,pitch:usize,bytes:usize,guard:usize,sentinel:u8,raw:bool)->Result<Vec<u8>,String> {
        if view.len()!=bytes||!view[..guard].iter().chain(&view[bytes-guard..]).all(|&v|v==sentinel) {
            return Err("readback length or prefix/suffix guards changed".into());
        }
        let mut rgb=Vec::with_capacity(width as usize*height as usize*if raw{4}else{3});
        for row in 0..height as usize {
            let at=guard+row*pitch;
            for rgba in view[at..at+width as usize*4].chunks_exact(4) {rgb.extend_from_slice(if raw{rgba}else{&rgba[..3]});}
            if !view[at+width as usize*4..at+pitch].iter().all(|&v|v==sentinel) {return Err("readback row padding guard changed".into());}
        }
        Ok(rgb)
    }
    fn mapped(width:u32,height:u32,padding:usize)->(Vec<u8>,usize) {
        let pitch=width as usize*4+padding;
        let mut view=vec![SENTINEL;GUARD*2+height as usize*pitch];
        for row in 0..height as usize {for column in 0..width as usize {
            let at=GUARD+row*pitch+column*4;
            view[at..at+4].copy_from_slice(&[(row*31+column*17) as u8,column as u8,row as u8,(row+column*3) as u8]);
        }}
        (view,pitch)
    }
    fn compare(view:&[u8],width:u32,height:u32,pitch:usize,bytes:usize,guard:usize,raw:bool) {
        let old=std::panic::catch_unwind(||original(view,width,height,pitch,bytes,guard,SENTINEL,raw));
        let new=std::panic::catch_unwind(||copy_guarded_rgba(view,width,height,pitch,bytes,guard,SENTINEL,raw));
        match(old,new) {(Ok(old),Ok(new))=>assert_eq!(old,new),(Err(_),Err(_))=>{},_=>panic!("reader result/panic contract changed")}
    }
    #[test]
    fn actual_npu_dimensions_preserve_every_rgba_byte_and_alpha() {
        let(view,pitch)=mapped(448,208,0);
        let bytes=copy_guarded_rgba(&view,448,208,pitch,view.len(),GUARD,SENTINEL,true).unwrap();
        assert_eq!(bytes.len(),448*208*4);
        assert_eq!(bytes,original(&view,448,208,pitch,view.len(),GUARD,SENTINEL,true).unwrap());
        assert_eq!(&bytes[4..8],&[17,1,0,3]);
    }
    #[test]
    fn raw_and_rgb_are_byte_exact_for_crop_sizes_and_padding() {
        for width in 0..9 {for height in 0..7 {for padding in [0,1,15,256] {for raw in [false,true] {
            let(view,pitch)=mapped(width,height,padding);
            compare(&view,width,height,pitch,view.len(),GUARD,raw);
        }}}}
    }
    #[test]
    fn rgb_debug_path_strips_only_alpha_in_original_pixel_order() {
        let(view,pitch)=mapped(3,2,7);
        let raw=copy_guarded_rgba(&view,3,2,pitch,view.len(),GUARD,SENTINEL,true).unwrap();
        let rgb=copy_guarded_rgba(&view,3,2,pitch,view.len(),GUARD,SENTINEL,false).unwrap();
        assert_eq!(rgb,raw.chunks_exact(4).flat_map(|rgba|rgba[..3].iter().copied()).collect::<Vec<_>>());
    }
    #[test]
    fn prefix_and_suffix_fail_before_pixel_or_padding_work() {
        let(view,pitch)=mapped(2,2,11);
        for index in [0,GUARD-1,view.len()-GUARD,view.len()-1] {for raw in [false,true] {
            let mut broken=view.clone();broken[index]^=1;
            assert_eq!(copy_guarded_rgba(&broken,2,2,pitch,broken.len(),GUARD,SENTINEL,raw).unwrap_err(),"readback length or prefix/suffix guards changed");
            compare(&broken,2,2,pitch,broken.len(),GUARD,raw);
        }}
    }
    #[test]
    fn each_row_padding_is_checked_without_including_it_in_output() {
        let(view,pitch)=mapped(3,4,5);
        for row in 0..4 {for pad in 0..5 {for raw in [false,true] {
            let mut broken=view.clone();broken[GUARD+row*pitch+12+pad]^=1;
            assert_eq!(copy_guarded_rgba(&broken,3,4,pitch,broken.len(),GUARD,SENTINEL,raw).unwrap_err(),"readback row padding guard changed");
            compare(&broken,3,4,pitch,broken.len(),GUARD,raw);
        }}}
    }
    #[test]
    fn length_mismatch_retains_short_circuit_error() {
        for view in [&[][..],&[SENTINEL][..]] {for raw in [false,true] {
            compare(view,448,208,1792,view.len()+1,GUARD,raw);
            assert!(copy_guarded_rgba(view,448,208,1792,view.len()+1,GUARD,SENTINEL,raw).is_err());
        }}
    }
    #[test]
    fn zero_geometry_and_existing_out_of_bounds_panics_match_reference() {
        let(view,pitch)=mapped(0,0,0);
        for raw in [false,true] {
            assert!(copy_guarded_rgba(&view,0,0,pitch,view.len(),GUARD,SENTINEL,raw).unwrap().is_empty());
            for (width,height,pitch,bytes,guard) in [(1,1,4,view.len(),GUARD),(4,2,1,view.len(),GUARD),(1,1,usize::MAX,view.len(),GUARD),(0,0,0,view.len(),view.len()+1)] {
                compare(&view,width,height,pitch,bytes,guard,raw);
            }
            compare(&[],0,0,0,0,GUARD,raw);
        }
    }
    #[test]
    fn output_owns_bytes_before_mapping_is_reused() {
        let(mut view,pitch)=mapped(5,3,9);
        let out=copy_guarded_rgba(&view,5,3,pitch,view.len(),GUARD,SENTINEL,true).unwrap();
        let expected=out.clone();view.fill(0);drop(view);assert_eq!(out,expected);
    }
}
