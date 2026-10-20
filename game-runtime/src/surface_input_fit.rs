//! Inverse coordinates for an aspect-fit guest image in a physical canvas.
//! Relative mouse-look deltas do not pass through this transform.

pub fn map_point(x: f64, y: f64, guest: (i32, i32), canvas: (u32, u32)) -> Option<(f64, f64)> {
    if !x.is_finite() || !y.is_finite() || guest.0 <= 0 || guest.1 <= 0 || canvas.0 == 0 || canvas.1 == 0 {
        return None;
    }
    let (gw, gh) = (guest.0 as f64, guest.1 as f64);
    let (cw, ch) = (canvas.0 as f64, canvas.1 as f64);
    let scale = (cw / gw).min(ch / gh);
    let left = (cw - gw * scale) * 0.5;
    let top = (ch - gh * scale) * 0.5;
    Some((((x - left) / scale).clamp(0.0, gw - 1.0),
          ((y - top) / scale).clamp(0.0, gh - 1.0)))
}

#[cfg(test)]
mod tests {
    use super::map_point;

    fn close(actual: (f64, f64), expected: (f64, f64)) {
        assert!((actual.0 - expected.0).abs() < 1e-8, "{actual:?} != {expected:?}");
        assert!((actual.1 - expected.1).abs() < 1e-8, "{actual:?} != {expected:?}");
    }

    #[test]
    fn matching_three_by_two_canvas() {
        close(map_point(1500.0, 1000.0, (1200, 800), (3000, 2000)).unwrap(), (600.0, 400.0));
        close(map_point(750.0, 500.0, (1200, 800), (3000, 2000)).unwrap(), (300.0, 200.0));
    }

    #[test]
    fn pillarbox_uses_center_offset_and_single_scale() {
        // 1200x800 occupies [250,2950]x[0,1800] on a 3200x1800 canvas.
        close(map_point(250.0, 0.0, (1200, 800), (3200, 1800)).unwrap(), (0.0, 0.0));
        close(map_point(1600.0, 900.0, (1200, 800), (3200, 1800)).unwrap(), (600.0, 400.0));
        close(map_point(1825.0, 1125.0, (1200, 800), (3200, 1800)).unwrap(), (700.0, 500.0));
        close(map_point(0.0, 900.0, (1200, 800), (3200, 1800)).unwrap(), (0.0, 400.0));
        close(map_point(3200.0, 900.0, (1200, 800), (3200, 1800)).unwrap(), (1199.0, 400.0));
    }

    #[test]
    fn letterbox_and_portrait_canvas() {
        // An aspect-fit 1600x900 image has 156.25 pixels above/below it.
        close(map_point(1500.0, 156.25, (1600, 900), (3000, 2000)).unwrap(), (800.0, 0.0));
        close(map_point(1500.0, 1000.0, (1600, 900), (3000, 2000)).unwrap(), (800.0, 450.0));
        close(map_point(1500.0, 2000.0, (1600, 900), (3000, 2000)).unwrap(), (800.0, 899.0));
        close(map_point(1000.0, 1500.0, (1200, 800), (2000, 3000)).unwrap(), (600.0, 400.0));
        close(map_point(1000.0, 0.0, (1200, 800), (2000, 3000)).unwrap(), (600.0, 0.0));
    }

    #[test]
    fn out_of_canvas_points_clamp_to_guest_pixel_bounds() {
        close(map_point(-100.0, -200.0, (1200, 800), (3000, 2000)).unwrap(), (0.0, 0.0));
        close(map_point(1e12, 1e12, (1200, 800), (3000, 2000)).unwrap(), (1199.0, 799.0));
        close(map_point(10.0, 10.0, (1, 1), (1, 1)).unwrap(), (0.0, 0.0));
    }

    #[test]
    fn invalid_values_are_rejected() {
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(map_point(v, 1.0, (1200, 800), (3000, 2000)).is_none());
            assert!(map_point(1.0, v, (1200, 800), (3000, 2000)).is_none());
        }
        for guest in [(0, 800), (1200, 0), (-1, 800)] {
            assert!(map_point(1.0, 1.0, guest, (3000, 2000)).is_none());
        }
        assert!(map_point(1.0, 1.0, (1200, 800), (0, 2000)).is_none());
        assert!(map_point(1.0, 1.0, (1200, 800), (3000, 0)).is_none());
    }
}
