#[path="spirv_layout_sanitizer.rs"] mod sanitizer;
fn records(v:&[sanitizer::ArrayStrideRecord])->String {
    format!("[{}]",v.iter().map(|r|format!("{{\"typeId\":{},\"stride\":{},\"storageClasses\":{:?}}}",r.type_id,r.stride,r.storage_classes)).collect::<Vec<_>>().join(","))
}
fn main()->Result<(),Box<dyn std::error::Error>> {
    let args:Vec<_>=std::env::args().collect();if args.len()!=3{return Err("usage: review_spirv_array_stride INPUT OUTPUT".into());}
    let bytes=std::fs::read(&args[1])?;if bytes.len()%4!=0{return Err("input length is not a multiple of four".into());}
    let words:Vec<u32>=bytes.chunks_exact(4).map(|b|u32::from_le_bytes(b.try_into().unwrap())).collect();
    let (out,report)=sanitizer::sanitize_array_stride(&words)?;
    let output:Vec<u8>=out.iter().flat_map(|w|w.to_le_bytes()).collect();std::fs::write(&args[2],output)?;
    println!("{{\"removed\":{},\"retainedMixed\":{},\"retainedOther\":{}}}",records(&report.removed),records(&report.retained_mixed),records(&report.retained_other));
    Ok(())
}
