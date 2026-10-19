//! Even spatial-upscaling dimensions fitted to the actual display aspect ratio.

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
