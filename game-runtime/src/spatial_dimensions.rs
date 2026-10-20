//! Even spatial-upscaling dimensions fitted to the actual display aspect ratio.

/// Explicit private sizes retain even extents and the guest's per-axis minimum.
pub(crate) fn valid_explicit_sizes(input:(u32,u32), output:(u32,u32)) -> bool {
    let bounded_even = |(w,h):(u32,u32)| w >= 2 && h >= 2 && w <= 8192 && h <= 8192 && w % 2 == 0 && h % 2 == 0;
    bounded_even(input) && bounded_even(output) && input.0 >= 320 && input.1 >= 240 &&
        output.0 >= input.0 && output.1 >= input.1 &&
        u64::from(input.0)*u64::from(output.1) == u64::from(input.1)*u64::from(output.0)
}

/// Select pixel sizes from the actual surface orientation and aspect ratio.
/// Short edges round UP to even; long edges round to nearest even (ties up),
/// so each long axis differs from the physical ratio by at most one pixel.
/// This is intentionally independent of the exact-ratio explicit-size gate:
/// two different short edges can require different integer-pixel rounding.
/// Portrait math is supported here; the current game ability remains landscape.
pub(crate) fn short_edge_sizes(native_short:u32,output_short:u32,display:(u32,u32)) -> Option<((u32,u32),(u32,u32))> {
    if display.0<2||display.1<2||native_short<800||output_short<1080||output_short<native_short{return None;}
    let size=|short:u32|->Option<(u32,u32)> {
        let short=short.checked_add(short&1)?;
        if short>8192{return None;}
        let physical_short=u64::from(display.0.min(display.1));
        let physical_long=u64::from(display.0.max(display.1));
        let numerator=physical_long.checked_mul(u64::from(short))?;
        let long=numerator.checked_add(physical_short)?.checked_div(physical_short.checked_mul(2)?)?.checked_mul(2)?;
        if long<u64::from(short)||long>8192{return None;}
        let long=u32::try_from(long).ok()?;
        Some(if display.0>=display.1{(long,short)}else{(short,long)})
    };
    let input=size(native_short)?;let output=size(output_short)?;
    // Same bound as the actual guest final-source RGBA8 snapshot path: 16 MiB.
    if u64::from(input.0)*u64::from(input.1)*4>16*1024*1024||
        output.0<input.0||output.1<input.1{return None;}
    Some((input,output))
}

/// Fits the display aspect ratio inside the selected mode's maximum dimensions.
///
/// Each dimension is rounded to its nearest even integer and stays within the
/// even bounds. Invalid display dimensions use the even bounds unchanged.
/// Bounds smaller than two are normalized to two, the minimum valid even size.
pub(crate) fn fit_aspect(bounds: (u32, u32), display: (u32, u32)) -> (u32, u32) {
    let width_bound = (bounds.0 & !1).max(2);
    let height_bound = (bounds.1 & !1).max(2);
    if display.0 < 2 || display.1 < 2 {
        return (width_bound, height_bound);
    }

    let display_width = f64::from(display.0);
    let display_height = f64::from(display.1);
    let scale = (f64::from(width_bound) / display_width)
        .min(f64::from(height_bound) / display_height);
    let nearest_even = |dimension: f64, bound: u32| -> u32 {
        ((dimension / 2.0).round() * 2.0)
            .clamp(2.0, f64::from(bound)) as u32
    };
    (
        nearest_even(display_width * scale, width_bound),
        nearest_even(display_height * scale, height_bound),
    )
}

#[cfg(test)]
mod tests {
    use super::fit_aspect;

    #[test]
    fn explicit_sr_sizes_keep_real_dimensions() {
        assert!(super::valid_explicit_sizes((960,540),(1920,1080)));
        assert!(super::valid_explicit_sizes((1280,720),(1920,1080)));
        assert!(super::valid_explicit_sizes((960,640),(1920,1280)));
        assert!(super::valid_explicit_sizes((1200,800),(1620,1080)));
        assert!(super::valid_explicit_sizes((800,800),(1080,1080)));
        assert!(super::valid_explicit_sizes((800,1200),(1080,1620)));
        for (input,output) in [((960,540),(1920,1082)),((961,540),(1922,1080)),
            ((960,540),(640,360)),((320,192),(1920,1080)),((960,540),(16384,9216)),((0,0),(0,0))] {
            assert!(!super::valid_explicit_sizes(input,output));
        }
    }

    #[test]
    fn short_edges_follow_actual_surface_and_orientation() {
        use super::short_edge_sizes as sizes;
        assert_eq!(sizes(800,1080,(3000,2000)),Some(((1200,800),(1620,1080))));
        assert_eq!(sizes(800,1080,(1920,1080)),Some(((1422,800),(1920,1080))));
        assert_eq!(sizes(800,1080,(3184,2232)),Some(((1142,800),(1540,1080))));
        assert_eq!(sizes(800,1080,(2000,3000)),Some(((800,1200),(1080,1620))));
        assert_eq!(sizes(800,1080,(2232,3184)),Some(((800,1142),(1080,1540))));
        assert_eq!(sizes(800,1080,(2000,2000)),Some(((800,800),(1080,1080))));
        assert_eq!(sizes(800,1080,(2400,1080)),Some(((1778,800),(2400,1080))));
        assert_eq!(sizes(801,1081,(3000,2000)),Some(((1204,802),(1624,1082))));
    }

    #[test]
    fn short_edges_reject_invalid_requests_and_snapshot_overflow() {
        use super::short_edge_sizes as sizes;
        for display in [(0,0),(1,2000),(3000,0),(u32::MAX,2),(2,u32::MAX)] {
            assert_eq!(sizes(800,1080,display),None);
        }
        for short in [(0,1080),(798,1080),(800,1078),(1200,1080),(8193,9000),(u32::MAX,u32::MAX)] {
            assert_eq!(sizes(short.0,short.1,(3000,2000)),None);
        }
        assert_eq!(sizes(2048,2048,(2000,2000)),Some(((2048,2048),(2048,2048))));
        assert_eq!(sizes(2050,2050,(2000,2000)),None);
    }

    #[test]
    fn short_edge_rounding_has_one_pixel_long_axis_error() {
        for display in [(3000,2000),(1920,1080),(3184,2232),(2232,3184),(2400,1080),(2000,2000),(3840,1600)] {
            let (input,output)=super::short_edge_sizes(800,1080,display).unwrap();
            for (size,short) in [(input,800),(output,1080)] {
                assert_eq!(size.0.min(size.1),short);
                assert_eq!(size.0%2,0);assert_eq!(size.1%2,0);
                let ideal=short as f64*display.0.max(display.1) as f64/display.0.min(display.1) as f64;
                assert!((size.0.max(size.1) as f64-ideal).abs()<=1.0);
            }
        }
    }

    #[test]
    fn current_mate_xt_display_uses_its_actual_aspect() {
        assert_eq!(fit_aspect((1202, 800), (3184, 2232)), (1142, 800));
        assert_eq!(fit_aspect((1802, 1200), (3184, 2232)), (1712, 1200));
        assert_eq!(fit_aspect((802, 534), (3184, 2232)), (762, 534));
    }

    #[test]
    fn familiar_landscape_and_portrait_aspects() {
        assert_eq!(fit_aspect((1202, 800), (3000, 2000)), (1200, 800));
        assert_eq!(fit_aspect((1202, 800), (3000, 1998)), (1202, 800));
        assert_eq!(fit_aspect((1202, 800), (1920, 1080)), (1202, 676));
        assert_eq!(fit_aspect((1202, 800), (2400, 1080)), (1202, 540));
        assert_eq!(fit_aspect((1202, 800), (1080, 1920)), (450, 800));
    }

    #[test]
    fn invalid_display_returns_even_bounds() {
        for display in [(0, 0), (1, 1), (0, 800), (1202, 0), (1, u32::MAX), (u32::MAX, 1)] {
            assert_eq!(fit_aspect((1203, 801), display), (1202, 800));
        }
        assert_eq!(fit_aspect((0, 1), (0, 0)), (2, 2));
    }

    #[test]
    fn extreme_dimensions_do_not_overflow_or_produce_zero() {
        assert_eq!(fit_aspect((1202, 800), (u32::MAX, 2)), (1202, 2));
        assert_eq!(fit_aspect((1202, 800), (2, u32::MAX)), (2, 800));
        assert_eq!(fit_aspect((u32::MAX, u32::MAX), (u32::MAX, u32::MAX)),
            (u32::MAX - 1, u32::MAX - 1));
        assert_eq!(fit_aspect((u32::MAX, 2), (2, u32::MAX)), (2, 2));
        assert_eq!(fit_aspect((1, 0), (3184, 2232)), (2, 2));
    }

    fn assert_dimensions(bounds: (u32, u32), display: (u32, u32)) {
        let result = fit_aspect(bounds, display);
        let even_bounds = ((bounds.0 & !1).max(2), (bounds.1 & !1).max(2));
        assert!(result.0 >= 2 && result.1 >= 2);
        assert!(result.0 <= even_bounds.0 && result.1 <= even_bounds.1);
        assert_eq!(result.0 % 2, 0);
        assert_eq!(result.1 % 2, 0);
        if display.0 < 2 || display.1 < 2 {
            assert_eq!(result, even_bounds);
            return;
        }
        // The fitted image fills at least one edge of its even bounding box.
        assert!(result.0 == even_bounds.0 || result.1 == even_bounds.1);
        let dw = f64::from(display.0);
        let dh = f64::from(display.1);
        let cross_error = (f64::from(result.0) * dh - f64::from(result.1) * dw).abs();
        // Nearest-even rounding changes either extent by at most one pixel;
        // an aspect too narrow for the two-pixel minimum can change it by two.
        let minimum_extent = (f64::from(even_bounds.0) / dw)
            .min(f64::from(even_bounds.1) / dh) * dw.min(dh);
        let pixel_error = if minimum_extent >= 2.0 { 1.0 } else { 2.0 };
        assert!(cross_error <= pixel_error * (dw + dh) + 0.01,
            "aspect error: bounds={bounds:?}, display={display:?}, result={result:?}");
    }

    #[test]
    fn property_sweep_even_bounded_and_aspect_error() {
        let values = [0, 1, 2, 3, 4, 7, 15, 16, 31, 100, 533, 534, 800, 1202,
            2232, 3184, 65535, 1_000_000, u32::MAX - 1, u32::MAX];
        let bounds = [(1202, 800), (1802, 1200), (802, 534), (1203, 801), (2, 2),
            (3, u32::MAX), (u32::MAX, 3), (u32::MAX, u32::MAX), (0, 1)];
        for bound in bounds {
            for width in values {
                for height in values {
                    assert_dimensions(bound, (width, height));
                }
            }
        }
        // A deterministic sweep covers nonstandard foldable/window dimensions.
        let mut state = 0x6d2b_79f5_u32;
        for index in 0..20_000_u32 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let width = state;
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let height = state;
            assert_dimensions(bounds[index as usize % bounds.len()], (width, height));
        }
    }
}
