use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Default)]
pub struct Keys {
    legacy: HashSet<String>, raw: HashMap<String, HashSet<String>>, caps: bool,
    events: HashMap<(String, String), ButtonSource>,
    blocked_legacy: HashSet<String>,
}
impl Keys {
    pub fn contains(&self, key: &str) -> bool { self.legacy.contains(key) || self.raw.values().any(|keys| keys.contains(key)) }
    pub fn legacy_contains(&self, key: &str) -> bool { self.legacy.contains(key) }
    pub fn legacy(&mut self, key: &str, pressed: bool) -> bool {
        if !pressed { self.blocked_legacy.remove(key); }
        else if self.blocked_legacy.contains(key) {return false;}
        if pressed { self.legacy.insert(key.into()); } else { self.legacy.remove(key); }
        true
    }
    pub fn raw(&mut self, key: &str, pressed: bool, source: &str) -> bool {
        let previous = self.raw.get(source).is_some_and(|keys| keys.contains(key));
        if pressed {
            self.raw.entry(source.into()).or_default().insert(key.into());
            if key == "CapsLock" && !previous { self.caps = !self.caps; }
        } else if let Some(keys) = self.raw.get_mut(source) {
            keys.remove(key); if keys.is_empty() { self.raw.remove(source); }
        }
        previous
    }
    pub fn raw_event(&mut self, key: &str, pressed: bool, source: &str, tick: u64) -> bool {
        let state=self.events.entry((source.into(),key.into())).or_default();
        let previous=state.logical_down;
        if previous==pressed {return previous;}
        state.logical_down=pressed;
        let set=if pressed && !state.engine_down && state.pending.is_empty() {
            state.engine_down=true;state.changed_at=tick;Some(true)
        } else if !pressed && state.engine_down && state.pending.is_empty() && tick.saturating_sub(state.changed_at)>=2 {
            state.engine_down=false;state.changed_at=tick;Some(false)
        } else {state.pending.push_back(pressed);None};
        if pressed && key=="CapsLock" {self.caps=!self.caps;}
        if let Some(down)=set {self.raw_set(key,down,source);}
        previous
    }
    fn raw_set(&mut self,key:&str,pressed:bool,source:&str) {
        if pressed {self.raw.entry(source.into()).or_default().insert(key.into());}
        else if let Some(keys)=self.raw.get_mut(source) {keys.remove(key);if keys.is_empty() {self.raw.remove(source);}}
    }
    pub fn guest_tick(&mut self, tick: u64) -> Vec<String> {
        let mut changes=Vec::new();
        for ((source,key),state) in &mut self.events {
            if state.advance(tick) {changes.push((source.clone(),key.clone(),state.engine_down));}
        }
        self.events.retain(|_,state| state.logical_down||state.engine_down||!state.pending.is_empty());
        for (source,key,down) in &changes {self.raw_set(key,*down,source);}
        changes.into_iter().map(|(_,key,_)| key).collect()
    }
    pub fn clear(&mut self) {
        // Physical autorepeat cannot turn a pre-invalidation Down into a new
        // gesture. Its corresponding Up is the only event that rearms it.
        self.blocked_legacy.extend(self.legacy.drain());
        self.raw.clear(); self.events.clear();
    }
    fn character_held(&self,key:&str)->bool {
        self.legacy.contains(key) || self.raw.iter().any(|(source,keys)| keys.contains(key) && self.events.get(&(source.clone(),key.to_string())).is_none_or(|state| state.logical_down))
    }
    pub fn ascii(&self, code: &str) -> Option<u16> {
        if ["ControlLeft","ControlRight","AltLeft","AltRight","MetaLeft","MetaRight"].iter().any(|key| self.character_held(key)) { return None; }
        let shift = self.character_held("ShiftLeft") || self.character_held("ShiftRight");
        if let Some(letter) = code.strip_prefix("Key").filter(|s| s.len() == 1 && s.as_bytes()[0].is_ascii_uppercase()) {
            let byte = letter.as_bytes()[0]; return Some(if shift ^ self.caps { byte } else { byte.to_ascii_lowercase() } as u16);
        }
        if let Some(digit) = code.strip_prefix("Digit").and_then(|s| s.parse::<usize>().ok()).filter(|n| *n < 10) {
            return Some(if shift { b")!@#$%^&*("[digit] } else { b'0'+digit as u8 } as u16);
        }
        if let Some(digit) = code.strip_prefix("Numpad").and_then(|s| s.parse::<u8>().ok()).filter(|n| *n < 10) { return Some((b'0'+digit) as u16); }
        let (normal, shifted) = match code {
            "Backquote" => (b'`',b'~'), "Minus" => (b'-',b'_'), "Equal" => (b'=',b'+'),
            "BracketLeft" => (b'[',b'{'), "BracketRight" => (b']',b'}'), "Backslash" => (b'\\',b'|'),
            "Semicolon" => (b';',b':'), "Quote" => (b'\'',b'"'), "Comma" => (b',',b'<'), "Period" => (b'.',b'>'), "Slash" => (b'/',b'?'),
            "Space" => (32,32), "Enter" | "NumpadEnter" => (13,13), "Backspace" => (8,8), "Tab" => (9,9),
            "NumpadAdd" => (b'+',b'+'), "NumpadSubtract" => (b'-',b'-'), "NumpadMultiply" => (b'*',b'*'),
            "NumpadDivide" => (b'/',b'/'), "NumpadDecimal" => (b'.',b'.'), _ => return None,
        };
        Some(if shift { shifted } else { normal } as u16)
    }
}

#[derive(Default)]
struct ButtonSource { logical_down:bool, engine_down:bool, changed_at:u64, pending:VecDeque<bool> }
impl ButtonSource {
    fn advance(&mut self,tick:u64)->bool {
        let minimum=if self.engine_down {2} else {1};
        if tick.saturating_sub(self.changed_at)>=minimum {
            if let Some(down)=self.pending.pop_front() {self.engine_down=down;self.changed_at=tick;return true;}
        }
        false
    }
}
#[derive(Default)]
pub struct Buttons { sources:HashMap<(String,usize),ButtonSource> }
impl Buttons {
    pub fn event(&mut self, button:usize, pressed:bool, source:&str, tick:u64) {
        let state=self.sources.entry((source.into(),button)).or_default();
        if source=="legacy" {state.logical_down=pressed;state.engine_down=pressed;state.pending.clear();return;}
        if state.logical_down==pressed {return;}
        state.logical_down=pressed;
        if pressed && !state.engine_down && state.pending.is_empty() {
            state.engine_down=true;state.changed_at=tick;
        } else {state.pending.push_back(pressed);}
    }
    pub fn guest_tick(&mut self,tick:u64) {
        for state in self.sources.values_mut() {state.advance(tick);}
        self.sources.retain(|_,state| state.logical_down||state.engine_down||!state.pending.is_empty());
    }
    pub fn down(&self,button:usize)->bool {self.sources.iter().any(|((_,b),state)| *b==button&&state.engine_down)}
    pub fn clear(&mut self) {self.sources.clear();}
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn releasing_virtual_key_preserves_physical_and_other_pointer_holds() {
        let mut k=Keys::default(); k.legacy("KeyW",true); k.raw("KeyW",true,"touch:1"); k.raw("KeyW",true,"touch:2");
        k.raw("KeyW",false,"touch:1"); assert!(k.contains("KeyW"));
        k.legacy("KeyW",false); assert!(k.contains("KeyW"));
        k.raw("KeyW",false,"touch:2"); assert!(!k.contains("KeyW"));
    }
    #[test] fn raw_keys_do_not_supply_legacy_flight_aliases() {
        let mut k=Keys::default(); k.raw("KeyI",true,"keyboard");
        assert!(k.contains("KeyI")); assert!(!k.legacy_contains("KeyI")); assert!(!k.contains("Numpad8"));
    }
    #[test] fn shift_caps_ascii_and_modifiers_are_independent_of_named_keys() {
        let mut k=Keys::default(); assert_eq!(k.ascii("KeyA"),Some(b'a' as u16));
        k.raw("ShiftRight",true,"shift"); assert_eq!(k.ascii("KeyA"),Some(b'A' as u16)); assert_eq!(k.ascii("Digit1"),Some(b'!' as u16));
        k.raw("CapsLock",true,"caps"); assert_eq!(k.ascii("KeyA"),Some(b'a' as u16));
        k.raw("ControlLeft",true,"ctrl"); assert_eq!(k.ascii("KeyA"),None);
        k.clear(); assert!(!k.contains("ShiftRight"));
    }
    #[test] fn quick_tap_survives_a_full_game_interval_then_releases() {
        let mut k=Keys::default(); k.raw_event("KeyH",true,"tap",10); k.raw_event("KeyH",false,"tap",10);
        assert!(k.contains("KeyH")); assert!(k.guest_tick(11).is_empty());
        assert_eq!(k.guest_tick(12),vec!["KeyH"]); assert!(!k.contains("KeyH"));
        k.raw_event("KeyH",true,"tap",13); k.raw_event("KeyH",false,"tap",13); k.clear();
        assert!(k.guest_tick(16).is_empty()); assert!(!k.contains("KeyH"));
    }
    #[test] fn repeated_fast_taps_deliver_each_ascii_down_before_guest_ticks() {
        let mut k=Keys::default(); let mut text=Vec::new();
        for pressed in [true,false,true,false] {
            let repeat=k.raw_event("KeyA",pressed,"keyboard:KeyA",10);
            if pressed && !repeat {text.push(k.ascii("KeyA").unwrap() as u8);}
        }
        assert_eq!(text,b"aa"); assert!(k.contains("KeyA"));
        k.guest_tick(12);assert!(!k.contains("KeyA"));
        k.guest_tick(13);assert!(k.contains("KeyA"));k.guest_tick(15);assert!(!k.contains("KeyA"));
        k.raw_event("CapsLock",true,"caps",13); k.raw_event("CapsLock",false,"caps",13);
        k.raw_event("CapsLock",true,"caps",13); k.raw_event("CapsLock",false,"caps",13);
        assert_eq!(k.ascii("KeyA"),Some(b'a' as u16));
    }
    #[test] fn physical_repeat_requires_up_after_global_invalidation() {
        let mut k=Keys::default(); assert!(k.legacy("KeyW",true)); k.clear();
        assert!(!k.legacy("KeyW",true)); assert!(!k.contains("KeyW"));
        k.clear();assert!(!k.legacy("KeyW",true));
        assert!(k.legacy("KeyW",false)); assert!(k.legacy("KeyW",true));assert!(k.contains("KeyW"));
    }
    #[test] fn raw_mouse_double_tap_preserves_edges_and_physical_ownership() {
        let mut b=Buttons::default();
        for down in [true,false,true,false] {b.event(0,down,"keyboard:mouse",10);}
        assert!(b.down(0));b.guest_tick(11);assert!(b.down(0));
        b.guest_tick(12);assert!(!b.down(0));b.guest_tick(13);assert!(b.down(0));
        b.event(0,true,"legacy",13);b.guest_tick(15);assert!(b.down(0));
        b.event(0,false,"legacy",15);assert!(!b.down(0));
        b.event(0,true,"keyboard:mouse",16);b.event(0,false,"keyboard:mouse",16);b.clear();
        b.guest_tick(18);assert!(!b.down(0));
    }
}
