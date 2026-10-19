//! Deterministic raw fixtures and comparison rules for the isolated GPU probe.
pub const PATTERNS: [&str; 8] = ["zero", "one", "ramp", "tile_edges", "special_f32_bits", "signed_zero_subnormal", "arbitrary_bits", "checker_extremes"];

fn mix(mut x: u32) -> u32 {
    x ^= x >> 16; x = x.wrapping_mul(0x7feb352d); x ^= x >> 15;
    x = x.wrapping_mul(0x846ca68b); x ^ (x >> 16)
}
pub fn input_words(width: u32, height: u32, pattern: usize, repeat: u32) -> (Vec<u8>, Vec<u8>) {
    let mut depth = Vec::with_capacity((width * height * 4) as usize);
    let mut auxiliary = Vec::with_capacity((width * height * 16) as usize);
    let special = [0,0x80000000,1,0x80000001,0x007fffff,0x00800000,0x3f000000,0x3f800000,
        0xbf800000,0x7f800000,0xff800000,0x7fc00001,0xffc12345,0x7f7fffff];
    for y in 0..height { for x in 0..width {
        let seed=mix(x.wrapping_mul(19349663)^y.wrapping_mul(73856093)^repeat.wrapping_mul(83492791));
        let bits=match pattern {
            0=>0, 1=>1.0f32.to_bits(),
            2=>((x+y)as f32/((width+height-2).max(1))as f32).to_bits(),
            3=>if x%16==0||y%16==0||x+1==width||y+1==height {1.0f32.to_bits()} else {0.25f32.to_bits()},
            4=>special[((x+y*3+repeat)as usize)%special.len()],
            5=>[0,0x80000000,1,0x80000001,0x007fffff,0x807fffff][((x+y+repeat)%6)as usize],
            6=>seed,
            _=>[0.0f32.to_bits(),1.0f32.to_bits(),(-1.0f32).to_bits(),2.0f32.to_bits()][((x^y^repeat)&3)as usize],
        };
        depth.extend_from_slice(&bits.to_le_bytes());
        // t8.x/w are consumed through integer bitcasts by the supplied shader.
        let (aux_x,aux_w)=if [0,1,2,3,7].contains(&pattern) {
            (((seed&1023)as f32/1023.0).to_bits(),(((seed>>10)&1023)as f32/1023.0).to_bits())
        } else {(seed,mix(seed^0x12345678))};
        for word in [aux_x,0u32,1.0f32.to_bits(),aux_w] {
            auxiliary.extend_from_slice(&word.to_le_bytes());
        }
    }}
    (depth,auxiliary)
}
pub fn uniform_words(width:u32,height:u32)->(Vec<u8>,Vec<u8>,Vec<u8>) {
    let mut cb10=vec![0u32;16];cb10[12]=width;cb10[13]=height;
    let mut cb11=vec![0u32;72];cb11[70]=1.0f32.to_bits();cb11[71]=0.125f32.to_bits();
    let mut constants=vec![0u32;24];
    for (i,value) in [1u32,4,8,12,16,128].into_iter().enumerate(){constants[i*4]=value;}
    let bytes=|words:Vec<u32>|words.into_iter().flat_map(u32::to_le_bytes).collect();
    (bytes(cb10),bytes(cb11),bytes(constants))
}
pub fn sentinel(bpp:u32)->Vec<u8> {
    if bpp==8 {[0x3555u16,0xb800,0x3bff,0x4900].into_iter().flat_map(u16::to_le_bytes).collect()}
    else {[0x3eaaaaabu32,0xbf000000,0x3f7fffff,0x41200000].into_iter().flat_map(u32::to_le_bytes).collect()}
}
pub fn groups(width:u32,height:u32)->(u32,u32){(width.div_ceil(16),height.div_ceil(16))}

#[cfg(test)] mod tests {
    use super::*;
    #[test]fn dimensions_raw_patterns_and_constant_layout_are_exact(){
        assert_eq!(groups(1202,800),(76,50));assert_eq!(groups(450,298),(29,19));
        let(a,b,c)=uniform_words(1202,800);assert_eq!((a.len(),b.len(),c.len()),(64,288,96));
        assert_eq!(&a[48..52],&1202u32.to_le_bytes());assert_eq!(&a[52..56],&800u32.to_le_bytes());
        for (i,n)in[1u32,4,8,12,16,128].into_iter().enumerate(){assert_eq!(&c[i*16..i*16+4],&n.to_le_bytes());assert!(c[i*16+4..i*16+16].iter().all(|&v|v==0));}
        for p in 0..PATTERNS.len(){let(a,b)=input_words(19,17,p,0);assert_eq!(a.len(),19*17*4);assert_eq!(b.len(),19*17*16);assert_eq!((a.clone(),b.clone()),input_words(19,17,p,0));}
        let(a,_)=input_words(19,17,4,0);assert!(a.chunks_exact(4).any(|b|b==0x7fc00001u32.to_le_bytes()));
        assert_eq!((sentinel(8).len(),sentinel(16).len()),(8,16));
    }
}
