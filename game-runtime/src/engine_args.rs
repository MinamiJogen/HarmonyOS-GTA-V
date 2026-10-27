//! Single source for the engine frame-queue argument. The default remains 3.
//! Canonicalization does not change swapchain depth or add a presentation wait.
pub(crate) fn normalize_frame_queue(args: &mut Vec<String>) -> u32 {
    let mut selected = 3;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        let (name, value) = arg.split_once('=').map_or((arg.as_str(), None), |(name, value)| (name, Some(value)));
        if !name.eq_ignore_ascii_case("-FrameQueueLimit") { index += 1; continue; }
        let separate = value.is_none() && args.get(index + 1).is_some_and(|v| !v.starts_with('-'));
        let number = value.or_else(|| separate.then(|| args[index + 1].as_str()))
            .and_then(|value| value.parse::<u32>().ok()).filter(|value| *value <= i32::MAX as u32);
        if let Some(value) = number { selected = value; }
        args.remove(index);
        if separate { args.remove(index); }
    }
    args.push(format!("-FrameQueueLimit={selected}"));
    selected
}

/// Unlimited native mode owns the coarse engine cap. Stale user/diagnostic
/// overrides cannot reintroduce a second -frameLimit after the canonical zero.
pub(crate) fn normalize_unlimited_frame_limit(args:&mut Vec<String>) {
    let mut index=0;
    while index<args.len() {
        let arg=&args[index];let (name,value)=arg.split_once('=').map_or((arg.as_str(),None),|(name,value)|(name,Some(value)));
        if !name.eq_ignore_ascii_case("-frameLimit") {index+=1;continue;}
        let separate=value.is_none()&&args.get(index+1).is_some_and(|value|!value.starts_with('-')||value.parse::<i64>().is_ok());
        args.remove(index);if separate{args.remove(index);}
    }
    args.push("-frameLimit=0".into());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unlimited_cap_is_unique_despite_stale_extra_arguments() {
        let mut args=["-frameLimit=2","-FRAMELIMIT","1","-frameLimit=-1","-frameLimitOther=9","-frameLimit","-other=42","-FrameQueueLimit=3"].map(String::from).to_vec();
        normalize_unlimited_frame_limit(&mut args);
        assert_eq!(args,["-frameLimitOther=9","-other=42","-FrameQueueLimit=3","-frameLimit=0"]);
        normalize_unlimited_frame_limit(&mut args);assert_eq!(args.last().unwrap(),"-frameLimit=0");assert_eq!(args.len(),4);
        assert_eq!(normalize_frame_queue(&mut args),3);
        assert_eq!(args.iter().filter(|a|a.as_str()=="-frameLimit=0").count(),1);
    }
    #[test]
    fn default_is_unchanged_and_exactly_one_argument_survives() {
        let mut args = vec!["-frameLimit=2".into(), "-FrameQueueLimitOther=keep".into()];
        assert_eq!(normalize_frame_queue(&mut args), 3);
        assert_eq!(args, ["-frameLimit=2", "-FrameQueueLimitOther=keep", "-FrameQueueLimit=3"]);
        assert_eq!(normalize_frame_queue(&mut args), 3);
        assert_eq!(args.len(), 3);
    }
    #[test]
    fn last_valid_override_wins_and_invalid_values_do_not_escape() {
        let mut args = ["-FrameQueueLimit=2", "-FRAMEQUEUELIMIT", "1", "-FrameQueueLimit=-1", "-FrameQueueLimit=4294967296", "-FrameQueueLimit=NaN", "-other"].map(String::from).to_vec();
        assert_eq!(normalize_frame_queue(&mut args), 1);
        assert_eq!(args, ["-other", "-FrameQueueLimit=1"]);
    }
    #[test]
    fn incomplete_option_does_not_consume_a_different_switch() {
        let mut args = ["-FrameQueueLimit", "-other=42"].map(String::from).to_vec();
        assert_eq!(normalize_frame_queue(&mut args), 3);
        assert_eq!(args, ["-other=42", "-FrameQueueLimit=3"]);
    }
}
