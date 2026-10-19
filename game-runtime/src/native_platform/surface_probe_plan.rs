//! Pure scheduling model for the independent native surface control.

pub const DEFAULT_FRAMES: u32 = 720;
pub const MAX_FRAMES: u32 = 10_000;
pub const SUBMISSIONS_PER_POLL: u32 = 3;

pub fn groups(frames: u32) -> Result<Vec<std::ops::Range<u32>>, &'static str> {
    if !(SUBMISSIONS_PER_POLL..=MAX_FRAMES).contains(&frames) {
        return Err("surface probe frames must be 3..10000");
    }
    Ok((0..frames).step_by(SUBMISSIONS_PER_POLL as usize)
        .map(|first| first..(first + SUBMISSIONS_PER_POLL).min(frames)).collect())
}

pub fn clear_color(frame: u32) -> [f64; 4] {
    let mut color = [0.0, 0.0, 0.0, 1.0];
    color[((frame / 60) % 3) as usize] = 0.25 + (frame % 60) as f64 / 80.0;
    color
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_720_frames_have_240_three_submission_batches() {
        let g = groups(DEFAULT_FRAMES).unwrap();
        assert_eq!(g.len(), 240);
        assert!(g.iter().all(|group| group.len() == 3));
        assert_eq!(g.into_iter().flatten().collect::<Vec<_>>(), (0..720).collect::<Vec<_>>());
    }

    #[test]
    fn every_valid_count_has_exact_frame_order_and_only_one_possible_tail() {
        for count in 3..=MAX_FRAMES {
            let g = groups(count).unwrap();
            assert_eq!(g.len() as u32, count.div_ceil(3));
            assert!(g.iter().take(g.len() - 1).all(|group| group.len() == 3));
            assert!((1..=3).contains(&g.last().unwrap().len()));
            assert_eq!(g.first().unwrap().start, 0);
            assert_eq!(g.last().unwrap().end, count);
            assert!(g.windows(2).all(|pair| pair[0].end == pair[1].start));
        }
    }

    #[test]
    fn rejects_counts_that_cannot_reach_three_submissions_or_exceed_budget() {
        for count in [0, 1, 2, MAX_FRAMES + 1, u32::MAX] { assert!(groups(count).is_err()); }
    }

    #[test]
    fn visible_clear_pattern_is_bounded_and_changes_all_rgb_phases() {
        for frame in 0..MAX_FRAMES {
            let c = clear_color(frame);
            assert!(c.iter().all(|v| (0.0..=1.0).contains(v)));
            assert_eq!(c[3], 1.0);
            assert_eq!(c[..3].iter().filter(|&&v| v > 0.0).count(), 1);
        }
        assert!(clear_color(0)[0] > 0.0);
        assert!(clear_color(60)[1] > 0.0);
        assert!(clear_color(120)[2] > 0.0);
    }
}
