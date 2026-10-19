#[path="spirv_layout_sanitizer.rs"]mod layout;
#[path="spirv_sample_mask_sanitizer.rs"]mod sample_mask;
fn main()->Result<(),Box<dyn std::error::Error>>{
 let a:Vec<_>=std::env::args().collect();if a.len()!=3{return Err("usage: review_spirv_vulkan INPUT OUTPUT".into());}
 let bytes=std::fs::read(&a[1])?;if bytes.len()%4!=0{return Err("non-word input".into());}
 let words:Vec<_>=bytes.chunks_exact(4).map(|b|u32::from_le_bytes(b.try_into().unwrap())).collect();
 let (layout_words,lr)=layout::sanitize_array_stride(&words)?;let(out,mr)=sample_mask::sanitize_sample_mask(&layout_words)?;
 std::fs::write(&a[2],out.iter().flat_map(|w|w.to_le_bytes()).collect::<Vec<_>>())?;
 println!("{{\"arrayStrideRemoved\":{},\"arrayStrideMixedRetained\":{},\"sampleMaskWrapped\":{},\"sampleMaskScalarAccesses\":{}}}",lr.removed.len(),lr.retained_mixed.len(),mr.len(),mr.iter().map(|r|r.accesses).sum::<usize>());Ok(())
}
