//! Read-only, bounded-rate game observations. ArkUI reads a published snapshot;
//! it never calls guest natives and never enables multiplayer to obtain state.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

pub const STALE_AFTER_MS: u64 = 500;
pub trait Query { fn integer(&mut self, name: &str, args: &[i32]) -> Result<i32, &'static str>; }
const FIELDS: &[&str] = &[
    "playerValid", "playerControlOn", "dead", "ragdoll", "enteringVehicle", "inVehicle",
    "driverSeat", "seatValid", "inMeleeCombat", "aiming", "inCover", "reloading", "preparingThrow",
    "cutscene", "cutsceneBlocksInput", "uiOwnsInput", "failureMenuOwnsInput", "turretInputOwned",
    "loading", "worldValid", "gameAllowsSkip", "gameShowsConfirm", "gameShowsCancel", "transport",
    "ui", "weaponClass", "parachutePhase", "vehicleKind", "vehicleSubtype", "startupStage", "photoStageId",
];

pub fn now_ms() -> u64 {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    EPOCH.get_or_init(Instant::now).elapsed().as_millis().min(u64::MAX as u128) as u64
}
fn unknown(source: &str, reason: &str, generation: u64, at: u64) -> Value {
    json!({"known":false,"reason":reason,"source":source,"generation":generation,"sampledAtMonoMs":at})
}
fn observed(value: impl Into<Value>, source: &str, generation: u64, at: u64) -> Value {
    json!({"known":true,"value":value.into(),"source":source,"generation":generation,"sampledAtMonoMs":at})
}
fn blank(generation: u64, seq: u64, at: u64, reason: &str) -> Value {
    let mut state = json!({"generation":generation,"seq":seq,"sampledAtMonoMs":at,
        "inputContextEpoch":generation,"queryAvailability":{},"specialCapabilities":{},
        "actionEligibility":{},"actionSignals":{},"prompts":[],"supportedControlGroups":[0,2],"taskPadVerified":false});
    for field in FIELDS { state[field] = unknown("guest:notConnected", reason, generation, at); }
    state["script"] = json!({
        "profileId":unknown("guest:notConnected",reason,generation,at),
        "stageId":unknown("guest:notConnected",reason,generation,at),
        "ownsInput":unknown("guest:notConnected",reason,generation,at)});
    state
}

/// Cache identical calls within one safe point, retaining failure as failure.
struct Sample<'a, Q> {
    query: &'a mut Q, cache: HashMap<(String, Vec<i32>), Result<i32, &'static str>>,
    state: Value, generation: u64, at: u64,
}
impl<Q: Query> Sample<'_, Q> {
    fn get(&mut self, name: &str, args: &[i32]) -> Result<i32, &'static str> {
        let result = *self.cache.entry((name.into(), args.into())).or_insert_with(|| self.query.integer(name, args));
        self.state["queryAvailability"][name] = Value::from(match result {
            Ok(_) => "available", Err("missingExport") => "missingExport", _ => "queryFailed",
        });
        result
    }
    fn put(&mut self, field: &str, value: impl Into<Value>, source: &str) {
        self.state[field] = observed(value, source, self.generation, self.at);
    }
    fn fail(&mut self, field: &str, source: &str, reason: &str) {
        self.state[field] = unknown(source, reason, self.generation, self.at);
    }
    fn boolean(&mut self, field: &str, name: &str, args: &[i32]) -> Option<bool> {
        match self.get(name, args) {
            Ok(value) => { let value = value != 0; self.put(field, value, name); Some(value) },
            Err(reason) => { self.fail(field, name, reason); None },
        }
    }
}

pub fn observe(query: &mut impl Query, generation: u64, seq: u64, at: u64) -> Value {
    let mut s = Sample { query, cache: HashMap::new(), state: blank(generation, seq, at, "notConnected"), generation, at };
    let pause = s.get("IsPauseMenuActive", &[]).ok().map(|n| n != 0);
    if pause == Some(true) { s.put("ui", "pause", "IsPauseMenuActive"); s.put("uiOwnsInput", true, "IsPauseMenuActive"); }
    let loading = s.boolean("loading", "IsLoadingScreenActive", &[]);
    let cutscene = s.boolean("cutscene", "IsCutsceneActive", &[]);
    if loading == Some(true) { s.put("startupStage", "loading", "IsLoadingScreenActive"); }
    let ped = match s.get("PlayerPedId", &[]) {
        Ok(0) => { s.put("playerValid", false, "PlayerPedId"); return s.state; },
        Ok(ped) => ped,
        Err(reason) => { s.fail("playerValid", "PlayerPedId", reason); return s.state; },
    };
    if s.boolean("playerValid", "DoesEntityExist", &[ped]) != Some(true) { return s.state; }
    let player = s.get("PlayerId", &[]).ok();
    let control = player.and_then(|player| s.boolean("playerControlOn", "IsPlayerControlOn", &[player]));
    if let Some(player) = player { s.boolean("aiming", "IsPlayerFreeAiming", &[player]); }
    for (field, name, args) in [
        ("dead", "IsPedDeadOrDying", vec![ped, 1]), ("ragdoll", "IsPedRagdoll", vec![ped]),
        ("inCover", "IsPedInCover", vec![ped, 0]), ("reloading", "IsPedReloading", vec![ped]),
        ("inMeleeCombat", "IsPedInMeleeCombat", vec![ped]),
    ] { s.boolean(field, name, &args); }
    if let Ok(vehicle) = s.get("GetVehiclePedIsTryingToEnter", &[ped]) { s.put("enteringVehicle", vehicle != 0, "GetVehiclePedIsTryingToEnter"); }
    let phone_task = s.get("IsPedRunningMobilePhoneTask", &[ped]).ok().map(|v| v != 0);
    // This identifies normal engine gameplay from actual control permission and
    // negative frontend observations. It does not identify phone/web/task pages.
    if pause == Some(false) && control == Some(true) && phone_task == Some(false) && cutscene == Some(false) {
        let source = "IsPauseMenuActive=false+IsPlayerControlOn=true+IsPedRunningMobilePhoneTask=false+IsCutsceneActive=false";
        s.put("ui", "gameplay", source); s.put("uiOwnsInput", false, source);
    }
    let in_vehicle = s.boolean("inVehicle", "IsPedInAnyVehicle", &[ped, 0]);
    let freefall = s.get("IsPedInParachuteFreefall", &[ped]).ok().map(|v| v != 0);
    let parachute = s.get("GetPedParachuteState", &[ped]).ok();
    // Only states with a verified native meaning are normalized. State 0 means
    // a packed parachute (not freefall); 3 is falling after a cut, not landing.
    match (freefall, parachute) {
        (Some(true), _) => s.put("parachutePhase", "freefall", "IsPedInParachuteFreefall"),
        (_, Some(1)) => s.put("parachutePhase", "opening", "GetPedParachuteState:1"),
        (_, Some(2)) => s.put("parachutePhase", "canopy", "GetPedParachuteState:2"),
        (Some(false), Some(-1 | 0)) => s.put("parachutePhase", "inactive", "GetPedParachuteState+IsPedInParachuteFreefall"),
        _ => {},
    }
    if freefall == Some(true) { s.put("transport", "freefall", "IsPedInParachuteFreefall"); }
    else if matches!(parachute, Some(1 | 2)) { s.put("transport", "parachute", "GetPedParachuteState"); }
    else if in_vehicle == Some(true) {
        if let Ok(veh) = s.get("GetVehiclePedIsIn", &[ped, 0]) {
            if veh != 0 && s.get("DoesEntityExist", &[veh]).is_ok_and(|value| value != 0) {
                s.put("seatValid", true, "IsPedInAnyVehicle+GetVehiclePedIsIn+DoesEntityExist");
                let driver = s.get("GetPedInVehicleSeat", &[veh, -1, 0]).ok().map(|driver| driver == ped);
                if let Some(driver) = driver { s.put("driverSeat", driver, "GetPedInVehicleSeat:-1"); }
                let model = s.get("GetEntityModel", &[veh]).ok().map(|m| m as u32);
                let subtype = model.and_then(|m| ["submersible", "submersible2"].into_iter().find(|name| joaat(name) == m));
                let sub = subtype.is_some();
                if let Some(subtype)=subtype {s.put("vehicleSubtype",subtype,"GetEntityModel:joaat");}
                let class = s.get("GetVehicleClass", &[veh]).ok();
                let kind = if sub { Some("submarine") } else if class == Some(14) && model.is_none() { None } else { class.and_then(vehicle_kind) };
                if let Some(kind) = kind { s.put("vehicleKind", kind, "GetEntityModel+GetVehicleClass"); }
                if driver == Some(false) { s.put("transport", "passenger", "GetPedInVehicleSeat:-1"); }
                else if driver == Some(true) { if let Some(kind) = kind { s.put("transport", kind, "GetPedInVehicleSeat+GetEntityModel+GetVehicleClass"); } }
            }
        }
    } else if in_vehicle == Some(false) {
        s.put("seatValid", false, "IsPedInAnyVehicle"); s.put("driverSeat", false, "IsPedInAnyVehicle");
        let underwater = s.get("IsPedSwimmingUnderWater", &[ped]).ok().map(|v| v != 0);
        let swimming = s.get("IsPedSwimming", &[ped]).ok().map(|v| v != 0);
        if underwater == Some(true) { s.put("transport", "swimUnderwater", "IsPedSwimmingUnderWater"); }
        else if swimming == Some(true) && underwater == Some(false) { s.put("transport", "swimSurface", "IsPedSwimming+IsPedSwimmingUnderWater"); }
        else if swimming == Some(false) && underwater == Some(false) && freefall == Some(false) && matches!(parachute, Some(-1 | 0)) {
            s.put("transport", "foot", "IsPedInAnyVehicle+IsPedSwimming+IsPedSwimmingUnderWater+IsPedInParachuteFreefall+GetPedParachuteState");
        }
    }
    if let Ok(weapon) = s.get("GetSelectedPedWeapon", &[ped]) {
        if weapon as u32 == joaat("WEAPON_UNARMED") { s.put("weaponClass", "unarmed", "GetSelectedPedWeapon"); }
        else if let Ok(group) = s.get("GetWeaponTypeGroup", &[weapon]) {
            if let Some(class) = weapon_class(group as u32) { s.put("weaponClass", class, "GetSelectedPedWeapon+GetWeaponTypeGroup"); }
            else { s.fail("weaponClass", "GetWeaponTypeGroup", "decodeUnverified"); }
        }
    }
    s.state
}
fn joaat(text: &str) -> u32 {
    let mut hash = 0u32;
    for byte in text.bytes() { hash = hash.wrapping_add(byte.to_ascii_lowercase() as u32); hash = hash.wrapping_add(hash << 10); hash ^= hash >> 6; }
    hash = hash.wrapping_add(hash << 3); hash ^= hash >> 11; hash.wrapping_add(hash << 15)
}
fn vehicle_kind(class: i32) -> Option<&'static str> {
    match class { 8 => Some("motorbike"), 13 => Some("bicycle"), 14 => Some("boat"), 15 => Some("heli"), 16 => Some("plane"),
        0..=7 | 9..=12 | 17..=20 | 22 => Some("car"), _ => None }
}
fn weapon_class(group: u32) -> Option<&'static str> {
    for (name, class) in [("GROUP_UNARMED","unarmed"),("GROUP_MELEE","melee"),("GROUP_THROWN","throwable"),("GROUP_SNIPER","sniper"),
        ("GROUP_PISTOL","firearm"),("GROUP_SMG","firearm"),("GROUP_RIFLE","firearm"),("GROUP_MG","firearm"),
        ("GROUP_SHOTGUN","firearm"),("GROUP_HEAVY","firearm"),("GROUP_STUNGUN","firearm")] {
        if group == joaat(name) { return Some(class); }
    }
    None
}

#[derive(Default)]
struct Published { lifecycle: u64, host: u64, generation: u64, seq: u64, at: Option<u64>, state: Option<Value> }
static PUBLISHED: Mutex<Published> = Mutex::new(Published { lifecycle:0,host:0,generation:0,seq:0,at:None,state:None });
impl Published {
    fn refresh_generation(&mut self, lifecycle: u64, host: Option<u64>) {
        if self.lifecycle != lifecycle || host.is_some_and(|host| self.host != host) {
            self.lifecycle = lifecycle; if let Some(host) = host { self.host = host; }
            self.generation = self.generation.wrapping_add(1); self.at = None; self.state = None;
        }
    }
    fn prepare(&mut self, lifecycle: u64, host: u64, at: u64) -> Option<(u64,u64)> {
        self.refresh_generation(lifecycle, Some(host));
        if self.at.is_some_and(|previous| at.saturating_sub(previous) < 50) { return None; }
        self.seq = self.seq.wrapping_add(1);
        Some((self.generation,self.seq))
    }
    #[cfg(test)]
    fn sample(&mut self, query: &mut impl Query, lifecycle: u64, host: u64, at: u64) {
        if let Some((generation,seq))=self.prepare(lifecycle,host,at) {
            self.state=Some(observe(query,generation,seq,at));self.at=Some(at);
        }
    }
    fn read(&mut self, lifecycle: u64, allowed: bool, now: u64) -> Value {
        self.refresh_generation(lifecycle, None);
        let mut state = if allowed { self.state.clone() } else { None }
            .unwrap_or_else(|| blank(self.generation, self.seq, 0, if allowed { "awaitingSafePoint" } else { "hostInvalid" }));
        state["nowMonoMs"] = now.into(); state["staleAfterMs"] = STALE_AFTER_MS.into();
        state["ageMs"] = self.at.map(|at| Value::from(now.saturating_sub(at))).unwrap_or(Value::Null);
        state
    }
}
pub fn sample(query: &mut impl Query, thread: u64) {
    if !crate::ffi::app_presentation_allowed() { return; }
    let lifecycle=crate::ffi::app_presentation_generation();let at=now_ms();
    let Some((generation,seq))=PUBLISHED.lock().unwrap().prepare(lifecycle,thread,at) else {return;};
    // Guest queries may take longer than a UI frame: never hold the publication
    // lock while executing them. Readers immediately obtain the previous copy.
    let state=observe(query,generation,seq,at);
    if !crate::ffi::app_presentation_allowed() || crate::ffi::app_presentation_generation()!=lifecycle {return;}
    let mut published=PUBLISHED.lock().unwrap();
    if published.generation==generation && published.lifecycle==lifecycle && published.host==thread {
        published.state=Some(state);published.at=Some(at);
    }
}
pub fn snapshot() -> Value {
    PUBLISHED.lock().unwrap().read(crate::ffi::app_presentation_generation(), crate::ffi::app_presentation_allowed(), now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)] struct Fake { values: HashMap<String, i32>, calls: HashMap<String, u32> }
    impl Query for Fake { fn integer(&mut self, name: &str, _: &[i32]) -> Result<i32, &'static str> {
        *self.calls.entry(name.into()).or_default() += 1; self.values.get(name).copied().ok_or("missingExport")
    } }
    fn foot() -> Fake {
        let mut f = Fake::default();
        for (name, value) in [("PlayerPedId",7),("DoesEntityExist",1),("PlayerId",0),("IsPlayerControlOn",1),
            ("IsPauseMenuActive",0),("IsPedRunningMobilePhoneTask",0),("IsCutsceneActive",0),("IsPedInAnyVehicle",0),
            ("IsPedSwimming",0),("IsPedSwimmingUnderWater",0),("IsPedInParachuteFreefall",0),("GetPedParachuteState",-1),
            ("GetSelectedPedWeapon",joaat("WEAPON_UNARMED") as i32)] { f.values.insert(name.into(),value); } f
    }
    #[test] fn missing_export_is_unknown_and_never_default_false() {
        let s = observe(&mut Fake::default(),3,1,100);
        assert_eq!(s["playerValid"]["known"],false); assert!(s["playerValid"].get("value").is_none());
        assert_eq!(s["queryAvailability"]["PlayerPedId"],"missingExport");
    }
    #[test] fn foot_gameplay_uses_fresh_actual_queries() {
        let s = observe(&mut foot(),3,2,100);
        assert_eq!(s["ui"]["value"],"gameplay"); assert_eq!(s["transport"]["value"],"foot");
        assert_eq!(s["weaponClass"]["value"],"unarmed"); assert_eq!(s["reloading"]["known"],false);
        assert_eq!(s["transport"]["sampledAtMonoMs"],100); assert_eq!(s["transport"]["generation"],3);
    }
    #[test] fn packed_chute_is_not_freefall_and_cut_chute_is_not_landing() {
        let mut f = foot(); f.values.insert("GetPedParachuteState".into(),0);
        assert_eq!(observe(&mut f,1,1,100)["transport"]["value"],"foot");
        f.values.insert("GetPedParachuteState".into(),3);
        let s = observe(&mut f,1,2,150); assert_eq!(s["parachutePhase"]["known"],false); assert_eq!(s["transport"]["known"],false);
    }
    #[test] fn phone_task_does_not_claim_a_phone_page_or_gameplay() {
        let mut f = foot(); f.values.insert("IsPedRunningMobilePhoneTask".into(),1);
        let s = observe(&mut f,1,1,100); assert_eq!(s["ui"]["known"],false); assert_eq!(s["uiOwnsInput"]["known"],false);
    }
    #[test] fn passenger_keeps_physical_vehicle_kind_and_missing_seat_stays_unknown() {
        let mut f=foot();
        for (name,value) in [("IsPedInAnyVehicle",1),("GetVehiclePedIsIn",11),("GetPedInVehicleSeat",99),
            ("GetVehicleClass",15),("GetEntityModel",joaat("maverick") as i32)] { f.values.insert(name.into(),value); }
        let s=observe(&mut f,1,1,100);
        assert_eq!(s["transport"]["value"],"passenger"); assert_eq!(s["vehicleKind"]["value"],"heli");
        f.values.remove("GetPedInVehicleSeat"); let s=observe(&mut f,1,2,150);
        assert_eq!(s["transport"]["known"],false); assert_eq!(s["driverSeat"]["known"],false);
        assert_eq!(s["vehicleKind"]["value"],"heli");
    }
    #[test] fn both_known_submarines_publish_their_verified_subtype() {
        for model in ["submersible","submersible2"] {
            let mut f=foot();
            for (name,value) in [("IsPedInAnyVehicle",1),("GetVehiclePedIsIn",11),("GetPedInVehicleSeat",7),
                ("GetVehicleClass",14),("GetEntityModel",joaat(model) as i32)] {f.values.insert(name.into(),value);}
            let s=observe(&mut f,4,2,100);
            assert_eq!(s["transport"]["value"],"submarine");assert_eq!(s["vehicleSubtype"]["value"],model);
            assert_eq!(s["vehicleSubtype"]["generation"],4);assert_eq!(s["vehicleSubtype"]["sampledAtMonoMs"],100);
        }
    }
    #[test] fn polling_does_not_refresh_observation_and_lifecycle_invalidates() {
        let mut p = Published::default(); let mut f = foot(); p.sample(&mut f,0,9,100);
        let a = p.read(0,true,200); let b = p.read(0,true,900);
        assert_eq!(a["sampledAtMonoMs"],100); assert_eq!(b["sampledAtMonoMs"],100); assert_eq!(b["ageMs"],800);
        let invalid = p.read(1,false,1000); assert_eq!(invalid["playerValid"]["known"],false);
        assert_ne!(invalid["generation"],a["generation"]);
        assert_eq!(p.read(2,true,1100)["playerValid"]["known"],false);
    }
    #[test] fn sampling_is_capped_and_missing_parachute_never_asserts_foot() {
        let mut p = Published::default(); let mut f = foot(); p.sample(&mut f,0,9,100); p.sample(&mut f,0,9,120);
        assert_eq!(p.seq,1); f.values.remove("GetPedParachuteState"); p.sample(&mut f,0,9,150);
        assert_eq!(p.seq,2); assert_eq!(p.state.as_ref().unwrap()["transport"]["known"],false);
    }
}
