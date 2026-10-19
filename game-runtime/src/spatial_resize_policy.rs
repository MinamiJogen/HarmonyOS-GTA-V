//! Eligibility gate for committing a requested spatial-upscaling size change.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct ResizeTarget {
    pub input: (u32, u32),
    pub output: (u32, u32),
    pub generation: u64,
}

/// The supplied guest engine ignores resize requests below this minimum.
pub(crate) fn valid_input((width, height): (u32, u32)) -> bool {
    width >= 320 && height >= 240
}

/// Commit only the current desired generation after its real source arrives.
/// Completion of all old GPU work is a separate caller-owned prerequisite.
pub(crate) fn ready(
    active: ResizeTarget,
    desired: ResizeTarget,
    observed: (u32, u32),
    blocked_generation: Option<u64>,
) -> bool {
    desired.generation != active.generation
        && (desired.input != active.input || desired.output != active.output)
        && observed == desired.input
        && valid_input(desired.input)
        && desired.output.0 >= desired.input.0
        && desired.output.1 >= desired.input.1
        && blocked_generation != Some(desired.generation)
}

#[cfg(test)]
mod tests {
    use super::{ready, valid_input, ResizeTarget};

    const ACTIVE: ResizeTarget = ResizeTarget {
        input: (1142, 800), output: (1712, 1200), generation: 1,
    };
    const DESIRED: ResizeTarget = ResizeTarget {
        input: (1202, 676), output: (1802, 1014), generation: 2,
    };

    #[test]
    fn waits_for_actual_new_source() {
        assert!(!ready(ACTIVE, DESIRED, ACTIVE.input, None));
        assert!(!ready(ACTIVE, DESIRED, (1202, 800), None));
        assert!(ready(ACTIVE, DESIRED, DESIRED.input, None));
    }

    #[test]
    fn committed_or_repeated_dimensions_do_not_trigger() {
        assert!(!ready(DESIRED, DESIRED, DESIRED.input, None));
        let repeated = ResizeTarget { generation: 3, ..DESIRED };
        assert!(!ready(DESIRED, repeated, repeated.input, None));
        let same_generation = ResizeTarget { generation: ACTIVE.generation, ..DESIRED };
        assert!(!ready(ACTIVE, same_generation, same_generation.input, None));
    }

    #[test]
    fn source_for_superseded_request_is_ignored() {
        let latest = ResizeTarget {
            input: (450, 800), output: (676, 1200), generation: 3,
        };
        assert!(!ready(ACTIVE, latest, DESIRED.input, None));
        assert!(!ready(ACTIVE, latest, ACTIVE.input, None));
        assert!(ready(ACTIVE, latest, latest.input, None));
    }

    #[test]
    fn failed_generation_is_blocked_even_if_source_arrives() {
        assert!(!ready(ACTIVE, DESIRED, ACTIVE.input, Some(DESIRED.generation)));
        assert!(!ready(ACTIVE, DESIRED, DESIRED.input, Some(DESIRED.generation)));
    }

    #[test]
    fn a_new_generation_can_retry_after_previous_failure() {
        let retry = ResizeTarget { generation: 3, ..DESIRED };
        assert!(ready(ACTIVE, retry, retry.input, Some(DESIRED.generation)));
        assert!(!ready(ACTIVE, retry, retry.input, Some(retry.generation)));
    }

    #[test]
    fn guest_minimum_is_enforced_per_axis() {
        assert!(valid_input((320, 240)));
        assert!(valid_input((u32::MAX, u32::MAX)));
        for input in [(0, 0), (1, 1), (319, 240), (320, 239), (319, u32::MAX), (u32::MAX, 239)] {
            assert!(!valid_input(input));
            let invalid = ResizeTarget { input, output: (u32::MAX, u32::MAX), generation: 2 };
            assert!(!ready(ACTIVE, invalid, input, None));
        }
    }

    #[test]
    fn output_must_not_downscale_either_axis() {
        for output in [(DESIRED.input.0 - 1, 1200), (1802, DESIRED.input.1 - 1), (0, 0)] {
            let invalid = ResizeTarget { output, ..DESIRED };
            assert!(!ready(ACTIVE, invalid, invalid.input, None));
        }
        let identity_output = ResizeTarget { output: DESIRED.input, ..DESIRED };
        assert!(ready(ACTIVE, identity_output, identity_output.input, None));
    }

    #[test]
    fn output_only_change_still_requires_new_generation() {
        let output_change = ResizeTarget {
            input: ACTIVE.input, output: (1802, 1200), generation: 2,
        };
        assert!(ready(ACTIVE, output_change, ACTIVE.input, None));
        let unversioned = ResizeTarget { generation: ACTIVE.generation, ..output_change };
        assert!(!ready(ACTIVE, unversioned, ACTIVE.input, None));
    }
}
