// Physical key codes from SDK 26 @ohos.multimodalInput.keyCode.
// Keep the legacy ASCII entry points for the existing native caller.
pub fn key_name(code: u32) -> Option<String> {
    let name = match code {
        13 | 2054 => "Enter", 27 | 2070 | 2 => "Escape", 32 | 2050 => "Space",
        8 | 2055 => "Backspace", 9 | 2049 => "Tab",
        2045 => "AltLeft", 2046 => "AltRight", 2047 => "ShiftLeft", 2048 => "ShiftRight",
        2072 => "ControlLeft", 2073 => "ControlRight", 2074 => "CapsLock", 2075 => "ScrollLock",
        2076 => "MetaLeft", 2077 => "MetaRight", 2079 => "PrintScreen", 2080 => "Pause",
        2081 => "Home", 2082 => "End", 2083 => "Insert", 2068 => "PageUp", 2069 => "PageDown",
        2071 => "Delete", 2102 => "NumLock", 2113 => "NumpadDivide", 2114 => "NumpadMultiply",
        2115 => "NumpadSubtract", 2116 => "NumpadAdd", 2117 => "NumpadDecimal", 2119 => "NumpadEnter",
        2043 => "Comma", 2044 => "Period", 2056 => "Backquote", 2057 => "Minus", 2058 => "Equal",
        2059 => "BracketLeft", 2060 => "BracketRight", 2061 => "Backslash", 2062 => "Semicolon",
        2063 => "Quote", 2064 => "Slash",
        2012 => "ArrowUp", 2013 => "ArrowDown", 2014 => "ArrowLeft", 2015 => "ArrowRight",
        2000..=2009 => return Some(format!("Digit{}", code - 2000)),
        2017..=2042 => return Some(format!("Key{}", char::from_u32(code - 2017 + 65)?)),
        2090..=2101 => return Some(format!("F{}", code - 2090 + 1)),
        2103..=2112 => return Some(format!("Numpad{}", code - 2103)),
        2816..=2822 => return Some(format!("F{}", code - 2816 + 13)),
        48..=57 => return Some(format!("Digit{}", code - 48)),
        65..=90 => return Some(format!("Key{}", char::from_u32(code)?)),
        97..=122 => return Some(format!("Key{}", char::from_u32(code - 32)?)),
        _ => return None,
    };
    Some(name.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independent_sdk_ranges_and_sides() {
        for (first, count, prefix, start) in [(2000, 10, "Digit", 0), (2090, 12, "F", 1),
            (2103, 10, "Numpad", 0), (2816, 7, "F", 13)] {
            for offset in 0..count { assert_eq!(key_name(first + offset), Some(format!("{prefix}{}", start + offset))); }
        }
        for offset in 0..26 { assert_eq!(key_name(2017 + offset), Some(format!("Key{}", (b'A' + offset as u8) as char))); }
        for (code, expected) in [(2045, "AltLeft"), (2046, "AltRight"), (2047, "ShiftLeft"),
            (2048, "ShiftRight"), (2072, "ControlLeft"), (2073, "ControlRight"),
            (2055, "Backspace"), (2071, "Delete"), (2119, "NumpadEnter"), (2070, "Escape")] {
            assert_eq!(key_name(code).as_deref(), Some(expected));
        }
        for code in [0, 2016, 2078, 2084, 2118, 4000, u32::MAX] { assert_eq!(key_name(code), None); }
    }
}
