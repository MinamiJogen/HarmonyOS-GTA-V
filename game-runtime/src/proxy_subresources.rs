//! Conservative selection of format-proxy mip/layer conversions. This module
//! changes no texture contents, formats, shader code or command ordering.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span { pub mip: u32, pub mips: u32, pub layer: u32, pub layers: u32 }
impl Span {
    pub fn one(mip: u32, layer: u32) -> Self { Self { mip, mips: 1, layer, layers: 1 } }
    fn valid(self, mips: u32, layers: u32) -> bool {
        self.mips != 0 && self.layers != 0
            && self.mip.checked_add(self.mips).is_some_and(|end| end <= mips)
            && self.layer.checked_add(self.layers).is_some_and(|end| end <= layers)
    }
}

#[derive(Clone, Copy, Debug)]
pub enum View { D2, D2Array, Cube, CubeArray }

/// The existing srv_view mapping, for known valid single-sampled D2 sources.
/// Unknown dimensions/invalid bases return None and retain whole-proxy work.
/// CubeArray's explicit count is already a layer count in the current consumer.
pub fn srv(mips: u32, layers: u32, skip: u32, a: u32, b: u32, c: u32, d: u32, view: View) -> Option<Span> {
    if mips == 0 || layers == 0 { return None; }
    let base = a.saturating_sub(skip);
    if base >= mips || c >= layers { return None; }
    let count = if skip == 0 || b == u32::MAX || b == 0 { b }
        else { b.saturating_sub(skip.saturating_sub(a)).max(1) };
    let mips = if count == 0 || count == u32::MAX { mips - base } else { count.min(mips - base).max(1) };
    let remaining = layers - c;
    if matches!(view, View::Cube | View::CubeArray) && remaining < 6 { return None; }
    let count = match view {
        View::D2 => 1,
        View::Cube if remaining < 6 => return None,
        View::Cube => 6,
        View::D2Array | View::CubeArray => if d == 0 { remaining } else { d },
    };
    let count = count.min(remaining).max(1);
    if matches!(view, View::CubeArray) && count % 6 != 0 { return None; }
    Some(Span { mip: base, mips, layer: c, layers: count })
}

pub struct Dirty {
    mips: u32,
    layers: u32,
    bits: Option<Vec<bool>>,
    whole: bool,
}
impl Dirty {
    pub fn new(mips: u32, layers: u32, enabled: bool) -> Self {
        // Hardware-valid arrays are much smaller. Invalid metadata must never
        // allocate an unbounded bitset or turn an unknown write into no work.
        let len = mips.checked_mul(layers).filter(|n| *n != 0 && *n <= 1 << 20);
        Self { mips, layers, bits: enabled.then_some(len).flatten().map(|n| vec![true; n as usize]), whole: true }
    }

    pub fn invalidate(&mut self, range: Option<Span>) {
        self.whole = true;
        if let Some(bits) = self.bits.as_mut() {
            if let Some(range) = range.filter(|range| range.valid(self.mips, self.layers)) {
                for mip in range.mip..range.mip + range.mips {
                    for layer in range.layer..range.layer + range.layers {
                        bits[(mip * self.layers + layer) as usize] = true;
                    }
                }
            } else { bits.fill(true); }
        }
    }

    /// Selection does not mutate validity. A failed/unrecorded conversion must
    /// not consume dirty work. Unknown reads use the old whole-proxy behavior.
    pub fn pending(&self, pass_count: usize, range: Option<Span>) -> Vec<usize> {
        if let Some(bits) = self.bits.as_ref() {
            if bits.len() == pass_count {
                if let Some(range) = range.filter(|range| range.valid(self.mips, self.layers)) {
                    return (range.mip..range.mip + range.mips).flat_map(|mip|
                        (range.layer..range.layer + range.layers).map(move |layer| (mip * self.layers + layer) as usize))
                        .filter(|&index| bits[index]).collect();
                }
                return if bits.iter().any(|dirty| *dirty) { (0..pass_count).collect() } else { Vec::new() };
            }
        }
        if self.whole { (0..pass_count).collect() } else { Vec::new() }
    }

    pub fn recorded(&mut self, passes: &[usize]) {
        if let Some(bits) = self.bits.as_mut() {
            for &pass in passes { if let Some(bit) = bits.get_mut(pass) { *bit = false; } }
            self.whole = bits.iter().any(|dirty| *dirty);
        } else if !passes.is_empty() { self.whole = false; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_read_only_consumes_its_mip_and_layers() {
        let mut dirty = Dirty::new(4, 3, true);
        let read = Span { mip: 1, mips: 2, layer: 1, layers: 1 };
        assert_eq!(dirty.pending(12, Some(read)), vec![4, 7]);
        assert_eq!(dirty.pending(12, Some(read)), vec![4, 7]); // no premature clean
        dirty.recorded(&[4, 7]);
        assert!(dirty.pending(12, Some(read)).is_empty());
        assert_eq!(dirty.pending(12, Some(Span::one(0, 0))), vec![0]);
        assert_eq!(dirty.pending(12, Some(Span::one(1, 2))), vec![5]);
    }
    #[test]
    fn exact_writes_do_not_invalidate_other_subresources_or_proxy_kinds() {
        let mut depth = Dirty::new(3, 2, true); let mut stencil = Dirty::new(3, 2, true);
        depth.recorded(&[0, 1, 2, 3, 4, 5]); stencil.recorded(&[0, 1, 2, 3, 4, 5]);
        depth.invalidate(Some(Span::one(1, 1)));
        assert_eq!(depth.pending(6, Some(Span::one(1, 1))), vec![3]);
        assert!(depth.pending(6, Some(Span::one(0, 1))).is_empty());
        assert!(stencil.pending(6, None).is_empty());
    }
    #[test]
    fn unknown_and_invalid_ranges_conservatively_use_whole_proxy_work() {
        let mut dirty = Dirty::new(2, 3, true); dirty.recorded(&[0, 1, 2, 3, 4, 5]);
        dirty.invalidate(Some(Span::one(1, 2))); assert_eq!(dirty.pending(6, None), vec![0, 1, 2, 3, 4, 5]);
        dirty.recorded(&[0, 1, 2, 3, 4, 5]);
        dirty.invalidate(Some(Span::one(u32::MAX, u32::MAX)));
        assert_eq!(dirty.pending(6, Some(Span::one(0, 0))), vec![0]);
        assert_eq!(dirty.pending(6, Some(Span { mip: 0, mips: 0, layer: 0, layers: 1 })), vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(Dirty::new(u32::MAX, 2, true).pending(3, None), vec![0, 1, 2]);
    }
    #[test]
    fn switch_off_preserves_whole_resource_conversions_for_known_views() {
        let mut dirty = Dirty::new(2, 3, false);
        assert_eq!(dirty.pending(6, Some(Span::one(1, 2))), vec![0, 1, 2, 3, 4, 5]);
        dirty.recorded(&[0, 1, 2, 3, 4, 5]); assert!(dirty.pending(6, None).is_empty());
        dirty.invalidate(Some(Span::one(0, 0)));
        assert_eq!(dirty.pending(6, Some(Span::one(1, 2))), vec![0, 1, 2, 3, 4, 5]);
    }
    #[test]
    fn srv_skips_open_counts_clamps_cube_faces_and_invalid_bases() {
        assert_eq!(srv(4, 12, 2, 0, 3, 0, 0, View::D2Array), Some(Span { mip: 0, mips: 1, layer: 0, layers: 12 }));
        for count in [0, u32::MAX] {
            assert_eq!(srv(4, 12, 2, 3, count, 2, 3, View::D2Array), Some(Span { mip: 1, mips: 3, layer: 2, layers: 3 }));
        }
        assert_eq!(srv(4, 12, 0, 2, 99, 6, 0, View::Cube), Some(Span { mip: 2, mips: 2, layer: 6, layers: 6 }));
        assert_eq!(srv(4, 12, 0, 0, 0, 0, 6, View::CubeArray).unwrap().layers, 6);
        assert!(srv(4, 12, 0, 0, 0, 7, 0, View::Cube).is_none());
        assert!(srv(4, 12, 0, u32::MAX, 0, 0, 0, View::D2).is_none());
        assert!(srv(4, 12, 0, 0, 0, u32::MAX, 0, View::D2Array).is_none());
        assert!(srv(0, 0, 0, 0, 0, 0, 0, View::D2).is_none());
    }
    #[test]
    fn randomized_read_write_content_matches_always_convert_reference() {
        let mut random = 0x3178_9261_4d89_019bu64;
        let mut dirty = Dirty::new(5, 7, true);
        let mut source = vec![0u64; 35]; let mut candidate = vec![u64::MAX; 35];
        let mut whole_reference = source.clone(); let mut converted = 0; let mut baseline = 0;
        for sequence in 1..25001 {
            random = random.wrapping_mul(6364136223846793005).wrapping_add(1);
            let mip = (random >> 13) as u32 % 5; let layer = (random >> 29) as u32 % 7;
            let span = Span::one(mip, layer);
            if random & 3 == 0 {
                source[(mip * 7 + layer) as usize] = sequence;
                dirty.invalidate(Some(span));
            } else if random & 31 == 1 {
                source.fill(sequence); dirty.invalidate(None);
            } else {
                whole_reference.copy_from_slice(&source); baseline += 35;
                let passes = dirty.pending(35, Some(span)); converted += passes.len();
                for &pass in &passes { candidate[pass] = source[pass]; }
                dirty.recorded(&passes);
                assert_eq!(candidate[(mip * 7 + layer) as usize], whole_reference[(mip * 7 + layer) as usize]);
            }
        }
        assert!(converted < baseline / 4, "model must exercise avoided work");
    }
}
