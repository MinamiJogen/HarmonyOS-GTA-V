// muguet - keyboard.rs
// Constantin Clerc (c22dev) - 0.1

use crate::input;
use objc2_game_controller::GCKeyboard;
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

static HELD: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());

const LETTERS: [&str; 26] = [
    "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ",
    "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX", "KeyY", "KeyZ",
];
const DIGITS: [&str; 10] = ["Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9", "Digit0"];
const FUNCTION: [&str; 12] = ["F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12"];
const KEYPAD: [&str; 10] = ["Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7", "Numpad8", "Numpad9", "Numpad0"];
const OTHERS: [(isize, &str); 35] = [
    (0x28, "Enter"), (0x29, "Escape"), (0x2A, "Backspace"), (0x2B, "Tab"), (0x2C, "Space"), (0x2D, "Minus"), (0x2E, "Equal"),
    (0x2F, "BracketLeft"), (0x30, "BracketRight"), (0x31, "Backslash"), (0x33, "Semicolon"), (0x34, "Quote"), (0x35, "Backquote"),
    (0x36, "Comma"), (0x37, "Period"), (0x38, "Slash"), (0x39, "CapsLock"), (0x49, "Insert"), (0x4A, "Home"), (0x4B, "PageUp"),
    (0x4C, "Delete"), (0x4D, "End"), (0x4E, "PageDown"), (0x4F, "ArrowRight"), (0x50, "ArrowLeft"), (0x51, "ArrowDown"), (0x52, "ArrowUp"),
    (0x58, "NumpadEnter"), (0x63, "NumpadDecimal"), (0xE0, "ControlLeft"), (0xE1, "ShiftLeft"), (0xE2, "AltLeft"), (0xE4, "ControlRight"),
    (0xE5, "ShiftRight"), (0xE6, "AltRight"),
];

fn keys() -> impl Iterator<Item = (isize, &'static str)> {
    let run = |first: isize, names: &'static [&'static str]| names.iter().enumerate().map(move |(i, n)| (first + i as isize, *n));
    run(0x04, &LETTERS).chain(run(0x1E, &DIGITS)).chain(run(0x3A, &FUNCTION)).chain(run(0x59, &KEYPAD)).chain(OTHERS)
}

pub fn poll() -> bool {
    let mut held = HELD.lock().unwrap();
    if !crate::pad::FOCUSED.load(Ordering::Relaxed) {
        held.clear();
        return false;
    }
    let Some(kb) = (unsafe { GCKeyboard::coalescedKeyboard().and_then(|k| k.keyboardInput()) }) else {
        for k in held.drain(..) { input::key(k, false, false, None, false); }
        return false;
    };
    if held.is_empty() && !unsafe { kb.isAnyKeyPressed() } { return true; }

    let down: HashSet<&'static str> =
        keys().filter(|(code, _)| unsafe { kb.buttonForKeyCode(*code).is_some_and(|b| b.isPressed()) }).map(|(_, n)| n).collect();
    held.retain(|k| {
        if !down.contains(k) { input::key(k, false, false, None, false); }
        down.contains(k)
    });
    for k in down {
        if !held.contains(&k) {
            crate::mp::key_pressed(k);
            input::key(k, true, false, None, false);
            held.push(k);
        }
    }
    true
}
