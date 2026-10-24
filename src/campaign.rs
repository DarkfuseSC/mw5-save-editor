//! Typed view over a parsed save: the handful of fields the editor exposes.
//! Field paths were confirmed against JJ's MW5 Merc Save Editor (MIT) and
//! verified on real saves. Everything is edited in place inside the
//! PropertyList tree so the lossless writer handles serialization.

use crate::io::{Reader, Writer};
use crate::prop::{ArrayBody, Property, PropertyList, Value};
use crate::save::{Archive, SaveFile};

pub const SKILLS: &[&str] = &[
    "Gunnery", "Ballistics", "Energy", "Missile",
    "Piloting", "Evasiveness", "Shielding", "HeatManagement",
];

pub const LOADOUT_MARKET: &str = "EMechLoadoutType::MarketLoadout";
pub const LOADOUT_BUYBACK: &str = "EMechLoadoutType::BuybackMarketLoadout";
pub const LOADOUT_COLD: &str = "EMechLoadoutType::ColdStorageLoadout";

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

pub fn get_path<'a>(pl: &'a PropertyList, path: &[&str]) -> Option<&'a Property> {
    let (first, rest) = path.split_first()?;
    let p = pl.get(first)?;
    if rest.is_empty() {
        return Some(p);
    }
    get_path(p.as_struct()?, rest)
}

pub fn get_path_mut<'a>(pl: &'a mut PropertyList, path: &[&str]) -> Option<&'a mut Property> {
    let (first, rest) = path.split_first()?;
    let p = pl.get_mut(first)?;
    if rest.is_empty() {
        return Some(p);
    }
    get_path_mut(p.as_struct_mut()?, rest)
}

fn set_scalar(p: &mut Property, bytes: Vec<u8>) -> bool {
    match &mut p.value {
        Value::Scalar { payload, .. } | Value::Enum { payload, .. } => {
            *payload = bytes;
            true
        }
        _ => false,
    }
}

fn fstring_bytes(s: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.fstring(s);
    w.into_bytes()
}

/// Display text of a TextProperty. Handles the two layouts MW5 uses for
/// pilot names (culture-invariant and namespaced); falls back to the last
/// printable FString in the payload.
pub fn read_text(payload: &[u8]) -> String {
    let mut r = Reader::new(payload);
    let structured = (|| -> Option<String> {
        r.i32().ok()?;
        match r.u8().ok()? {
            0xff => {
                if r.i32().ok()? != 0 { r.fstring().ok() } else { Some(String::new()) }
            }
            0x00 => {
                r.fstring().ok()?;
                r.fstring().ok()?;
                r.fstring().ok()
            }
            _ => None,
        }
    })();
    if let Some(s) = structured {
        return s;
    }
    let mut last = String::new();
    let mut i = 0usize;
    while i + 4 <= payload.len() {
        let n = i32::from_le_bytes(payload[i..i + 4].try_into().unwrap());
        if n > 0 && (n as usize) < 1024 && i + 4 + n as usize <= payload.len() && payload[i + 4 + n as usize - 1] == 0 {
            let s = &payload[i + 4..i + 4 + n as usize - 1];
            if s.iter().all(|b| (32..127).contains(b)) {
                last = s.iter().map(|&b| b as char).collect();
                i += 4 + n as usize;
                continue;
            }
        }
        i += 1;
    }
    last
}

/// Culture-invariant TextProperty: flags=2, history=0xff, has-invariant=1, FString.
pub fn build_text(s: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.i32(2);
    w.u8(0xff);
    w.i32(1);
    w.fstring(s);
    w.into_bytes()
}

// ---------------------------------------------------------------------------
// Date helpers (UE ticks = 100ns since 0001-01-01)
// ---------------------------------------------------------------------------

pub fn ticks_to_days(ticks: i64) -> i64 {
    ticks / (10_000_000i64 * 86_400)
}

/// days since 0001-01-01 -> (y, m, d), proleptic Gregorian.
pub fn civil_from_days(days_since_0001: i64) -> (i64, u32, u32) {
    // shift to days since 1970-01-01 then use Howard Hinnant's algorithm
    let z = days_since_0001 - 719_162 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

// ---------------------------------------------------------------------------
// Mech / Pilot / Faction views
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct MechInfo {
    pub index: usize,
    pub chassis: String,
    pub custom_name: String,
    pub loadout_type: String,
}

impl MechInfo {
    pub fn is_owned(&self) -> bool {
        !matches!(self.loadout_type.as_str(), LOADOUT_MARKET | LOADOUT_BUYBACK | LOADOUT_COLD)
    }
    pub fn category(&self) -> &'static str {
        match self.loadout_type.as_str() {
            LOADOUT_MARKET => "Market",
            LOADOUT_BUYBACK => "Buyback",
            LOADOUT_COLD => "Cold storage",
            _ => "Active bay",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PilotInfo {
    pub index: usize,
    pub callsign: String,
    pub full_name: String,
    pub is_commander: bool,
    pub salary: i32,
    /// TotalExp per SKILLS entry.
    pub skills: Vec<i32>,
}

#[derive(Clone, Debug)]
pub struct FactionInfo {
    pub index: usize,
    pub name: String,
    pub standing: i32,
}

/// One row of Coyote's Mission Pack save state (`CoyoteGlobals_C` persistent
/// actor, `CoyoteSaveGameConfig` array). Blueprint struct members are named
/// `id_19_<guid>`, `stringVal_6_<guid>` etc., so they are matched by prefix.
#[derive(Clone, Debug)]
pub struct CoyoteEntry {
    pub index: usize,
    pub id: String,
    pub string_val: String,
    pub float_val: f32,
    pub bool_val: bool,
    pub int_val: i32,
}

pub const COYOTE_ACTOR_CLASS: &str = "/Coyotesmission/functions/CoyoteGlobals.CoyoteGlobals_C";

fn field_by_prefix<'a>(pl: &'a PropertyList, prefix: &str) -> Option<&'a Property> {
    pl.props.iter().find(|p| p.name.starts_with(prefix))
}
fn field_by_prefix_mut<'a>(pl: &'a mut PropertyList, prefix: &str) -> Option<&'a mut Property> {
    pl.props.iter_mut().find(|p| p.name.starts_with(prefix))
}

/// A persistent actor record (`SaveStateModel.PersistentActors[i]`): mods and
/// base-game systems each own one, with their state in a nested archive.
#[derive(Clone, Debug)]
pub struct ActorInfo {
    pub index: usize,
    pub class: String,
    pub bytes: usize,
}

/// A save with its mech wrapper archives decoded so they can be edited and
/// written back. Call `flush()` before `SaveFile::to_bytes`.
pub struct Campaign {
    pub save: SaveFile,
    /// Parsed ByteData archives, index = position in MechLoadoutWrappers.
    wrappers: Vec<Option<Archive>>,
    wrappers_dirty: bool,
    /// Parsed PersistentActors ByteData archives, by array index.
    actors: Vec<(String, Option<Archive>)>,
    actors_dirty: bool,
}

impl Campaign {
    pub fn open(data: &[u8]) -> crate::io::Result<Campaign> {
        let mut save = SaveFile::parse(data)?;
        let mut wrappers = Vec::new();
        if let Ok(m) = save.model("SaveStateModel") {
            if let Some(els) = m.props.get("MechLoadoutWrappers").and_then(|p| p.as_struct_array()) {
                for el in els {
                    let arc = el.get("ByteData").and_then(|bd| match &bd.value {
                        Value::Array { body: ArrayBody::Raw(b), .. } => Archive::parse(b).ok(),
                        _ => None,
                    });
                    wrappers.push(arc);
                }
            }
        }
        let mut actors = Vec::new();
        if let Ok(m) = save.model("SaveStateModel") {
            if let Some(els) = m.props.get("PersistentActors").and_then(|p| p.as_struct_array()) {
                for el in els {
                    let class = el.get("ActorClass").and_then(|p| p.as_fstring()).unwrap_or_default();
                    let arc = el.get("ByteData").and_then(|bd| match &bd.value {
                        Value::Array { body: ArrayBody::Raw(b), .. } => Archive::parse(b).ok(),
                        _ => None,
                    });
                    actors.push((class, arc));
                }
            }
        }
        Ok(Campaign { save, wrappers, wrappers_dirty: false, actors, actors_dirty: false })
    }

    fn write_back(model_props: &mut PropertyList, array: &str, archives: impl Iterator<Item = Option<Vec<u8>>>) {
        let Some(w) = model_props.get_mut(array) else { return };
        let Value::Array { body: ArrayBody::Structs { elements, .. }, .. } = &mut w.value else { return };
        for (el, bytes) in elements.iter_mut().zip(archives) {
            let Some(bytes) = bytes else { continue };
            if let Some(bd) = el.get_mut("ByteData") {
                if let Value::Array { body: ArrayBody::Raw(b), .. } = &mut bd.value {
                    *b = bytes;
                }
            }
        }
    }

    /// Write edited wrapper and actor archives back into their ByteData blobs.
    pub fn flush(&mut self) {
        if !self.wrappers_dirty && !self.actors_dirty {
            return;
        }
        let wrapper_bytes: Vec<Option<Vec<u8>>> = if self.wrappers_dirty { self.wrappers.iter().map(|a| a.as_ref().map(|a| a.to_bytes())).collect() } else { Vec::new() };
        let actor_bytes: Vec<Option<Vec<u8>>> = if self.actors_dirty { self.actors.iter().map(|(_, a)| a.as_ref().map(|a| a.to_bytes())).collect() } else { Vec::new() };
        let Ok(m) = self.save.model("SaveStateModel") else { return };
        if self.wrappers_dirty {
            Self::write_back(&mut m.props, "MechLoadoutWrappers", wrapper_bytes.into_iter());
        }
        if self.actors_dirty {
            Self::write_back(&mut m.props, "PersistentActors", actor_bytes.into_iter());
        }
        self.wrappers_dirty = false;
        self.actors_dirty = false;
    }

    // -- persistent actors / Coyote's Mission Pack -------------------------
    pub fn actors(&self) -> Vec<ActorInfo> {
        self.actors.iter().enumerate().map(|(i, (class, arc))| ActorInfo {
            index: i, class: class.clone(),
            bytes: arc.as_ref().map(|a| a.to_bytes().len()).unwrap_or(0),
        }).collect()
    }

    fn coyote_rows(&self) -> Option<&Vec<PropertyList>> {
        let (_, arc) = self.actors.iter().find(|(c, _)| c == COYOTE_ACTOR_CLASS)?;
        arc.as_ref()?.props.get("CoyoteSaveGameConfig")?.as_struct_array()
    }
    fn coyote_rows_mut(&mut self) -> Option<&mut Vec<PropertyList>> {
        let (_, arc) = self.actors.iter_mut().find(|(c, _)| c == COYOTE_ACTOR_CLASS)?;
        match &mut arc.as_mut()?.props.get_mut("CoyoteSaveGameConfig")?.value {
            Value::Array { body: ArrayBody::Structs { elements, .. }, .. } => Some(elements),
            _ => None,
        }
    }

    pub fn has_coyote(&self) -> bool {
        self.coyote_rows().is_some()
    }

    pub fn coyote_entries(&self) -> Vec<CoyoteEntry> {
        let Some(rows) = self.coyote_rows() else { return Vec::new() };
        rows.iter().enumerate().map(|(i, el)| CoyoteEntry {
            index: i,
            id: field_by_prefix(el, "id_").and_then(|p| p.as_fstring()).unwrap_or_default(),
            string_val: field_by_prefix(el, "stringVal_").and_then(|p| p.as_fstring()).unwrap_or_default(),
            float_val: field_by_prefix(el, "floatVal_").and_then(|p| p.as_f32()).unwrap_or(0.0),
            bool_val: field_by_prefix(el, "boolVal_").map(|p| matches!(p.value, Value::Bool { value, .. } if value != 0)).unwrap_or(false),
            int_val: field_by_prefix(el, "intVal_").and_then(|p| p.as_i32()).unwrap_or(0),
        }).collect()
    }

    /// Update one Coyote row. Only fields that differ are touched.
    pub fn set_coyote_entry(&mut self, index: usize, string_val: &str, float_val: f32, bool_val: bool, int_val: i32) -> bool {
        let Some(rows) = self.coyote_rows_mut() else { return false };
        let Some(el) = rows.get_mut(index) else { return false };
        let mut changed = false;
        if let Some(p) = field_by_prefix_mut(el, "stringVal_") {
            if p.as_fstring().as_deref() != Some(string_val) { changed |= set_scalar(p, fstring_bytes(string_val)); }
        }
        if let Some(p) = field_by_prefix_mut(el, "floatVal_") {
            if p.as_f32() != Some(float_val) { changed |= set_scalar(p, float_val.to_le_bytes().to_vec()); }
        }
        if let Some(p) = field_by_prefix_mut(el, "boolVal_") {
            if let Value::Bool { value, .. } = &mut p.value {
                let nv = if bool_val { 1 } else { 0 };
                if *value != nv { *value = nv; changed = true; }
            }
        }
        if let Some(p) = field_by_prefix_mut(el, "intVal_") {
            if p.as_i32() != Some(int_val) { changed |= set_scalar(p, int_val.to_le_bytes().to_vec()); }
        }
        self.actors_dirty |= changed;
        changed
    }

    pub fn to_bytes(&mut self) -> Vec<u8> {
        self.flush();
        self.save.to_bytes()
    }

    // -- C-Bills / reputation ---------------------------------------------
    pub fn cbills(&mut self) -> Option<i64> {
        let m = self.save.model("InventoryModel").ok()?;
        get_path(&m.props, &["AvailableCBills", "Value"])?.as_i64()
    }
    pub fn set_cbills(&mut self, v: i64) -> bool {
        let Ok(m) = self.save.model("InventoryModel") else { return false };
        get_path_mut(&mut m.props, &["AvailableCBills", "Value"]).map(|p| set_scalar(p, v.to_le_bytes().to_vec())).unwrap_or(false)
    }
    pub fn reputation(&mut self) -> Option<i32> {
        self.save.model("MercCompanyModel").ok()?.props.get("Reputation")?.as_i32()
    }
    pub fn set_reputation(&mut self, v: i32) -> bool {
        let Ok(m) = self.save.model("MercCompanyModel") else { return false };
        m.props.get_mut("Reputation").map(|p| set_scalar(p, v.to_le_bytes().to_vec())).unwrap_or(false)
    }

    // -- timeline ----------------------------------------------------------
    pub fn start_ticks(&mut self) -> Option<i64> {
        self.save.model("TimelineModel").ok()?.props.get("StartingDateTicks")?.as_i64()
    }
    pub fn days_elapsed(&mut self) -> Option<i32> {
        self.save.model("TimelineModel").ok()?.props.get("TotalTimeElapsed")?.as_i32()
    }
    pub fn set_days_elapsed(&mut self, v: i32) -> bool {
        let Ok(m) = self.save.model("TimelineModel") else { return false };
        m.props.get_mut("TotalTimeElapsed").map(|p| set_scalar(p, v.to_le_bytes().to_vec())).unwrap_or(false)
    }
    /// Current in-game date as "YYYY.MM.DD".
    pub fn date_text(&mut self) -> String {
        let (Some(t), Some(d)) = (self.start_ticks(), self.days_elapsed()) else { return String::new() };
        let (y, m, dd) = civil_from_days(ticks_to_days(t) + d as i64);
        format!("{:04}.{:02}.{:02}", y, m, dd)
    }

    // -- mechs -------------------------------------------------------------
    pub fn mechs(&self) -> Vec<MechInfo> {
        let mut out = Vec::new();
        for (i, arc) in self.wrappers.iter().enumerate() {
            let Some(arc) = arc else { continue };
            let Some(mi) = arc.props.get("MarketItemMech") else { continue };
            let Some(mi) = mi.as_struct() else { continue };
            let chassis = get_path(mi, &["Item", "ItemData", "MechDataAssetId", "ID", "PrimaryAssetName"])
                .and_then(|p| p.as_fstring()).unwrap_or_default();
            let custom_name = get_path(mi, &["Item", "ItemData", "CustomName"])
                .and_then(|p| p.as_fstring()).unwrap_or_default();
            let loadout_type = arc.props.get("MechLoadoutType").and_then(|p| p.as_fstring()).unwrap_or_default();
            out.push(MechInfo { index: i, chassis, custom_name, loadout_type });
        }
        out
    }

    pub fn set_mech_custom_name(&mut self, index: usize, name: &str) -> bool {
        let Some(Some(arc)) = self.wrappers.get_mut(index) else { return false };
        let Some(mi) = arc.props.get_mut("MarketItemMech").and_then(|p| p.as_struct_mut()) else { return false };
        let Some(item_data) = get_path_mut(mi, &["Item", "ItemData"]).and_then(|p| p.as_struct_mut()) else { return false };
        if let Some(p) = item_data.get_mut("CustomName") {
            let ok = set_scalar(p, fstring_bytes(name));
            self.wrappers_dirty |= ok;
            return ok;
        }
        // Stock mechs omit CustomName; graft a StrProperty after MechDataAssetId.
        let prop = Property { name: "CustomName".into(), ptype: "StrProperty".into(), value: Value::Scalar { trailing: 0, payload: fstring_bytes(name) } };
        let pos = item_data.props.iter().position(|p| p.name == "MechDataAssetId").map(|i| i + 1).unwrap_or(item_data.props.len());
        item_data.props.insert(pos, prop);
        self.wrappers_dirty = true;
        true
    }

    // -- pilots ------------------------------------------------------------
    fn roster(&mut self) -> Option<&mut Vec<PropertyList>> {
        let m = self.save.model("RosterModel").ok()?;
        match &mut m.props.get_mut("PilotRoster")?.value {
            Value::Array { body: ArrayBody::Structs { elements, .. }, .. } => Some(elements),
            _ => None,
        }
    }

    pub fn pilots(&mut self) -> Vec<PilotInfo> {
        let Some(roster) = self.roster() else { return Vec::new() };
        roster.iter().enumerate().map(|(i, el)| {
            let persona = el.get("PersonaData").and_then(|p| p.as_struct());
            let callsign = persona.and_then(|p| p.get("Callsign")).and_then(|p| p.payload()).map(read_text).unwrap_or_default();
            let full_name = persona.and_then(|p| p.get("FullName")).and_then(|p| p.payload()).map(read_text).unwrap_or_default();
            let is_commander = el.get("LockedPlayerSlot").and_then(|p| p.as_fstring()).map(|s| !s.ends_with("::None")).unwrap_or(false);
            let salary = el.get("SalaryCBills").and_then(|p| p.as_i32()).unwrap_or(0);
            let skills = SKILLS.iter().map(|s| get_path(el, &["Skills", s, "TotalExp"]).and_then(|p| p.as_i32()).unwrap_or(0)).collect();
            PilotInfo { index: i, callsign, full_name, is_commander, salary, skills }
        }).collect()
    }

    pub fn set_pilot_skill(&mut self, index: usize, skill: &str, exp: i32) -> bool {
        let Some(roster) = self.roster() else { return false };
        let Some(el) = roster.get_mut(index) else { return false };
        get_path_mut(el, &["Skills", skill, "TotalExp"]).map(|p| set_scalar(p, exp.to_le_bytes().to_vec())).unwrap_or(false)
    }

    pub fn set_pilot_salary(&mut self, index: usize, v: i32) -> bool {
        let Some(roster) = self.roster() else { return false };
        let Some(el) = roster.get_mut(index) else { return false };
        el.get_mut("SalaryCBills").map(|p| set_scalar(p, v.to_le_bytes().to_vec())).unwrap_or(false)
    }

    pub fn set_pilot_callsign(&mut self, index: usize, s: &str) -> bool {
        let Some(roster) = self.roster() else { return false };
        let Some(el) = roster.get_mut(index) else { return false };
        get_path_mut(el, &["PersonaData", "Callsign"]).map(|p| set_scalar(p, build_text(s))).unwrap_or(false)
    }

    // -- factions ----------------------------------------------------------
    fn standings(&mut self) -> Option<&mut Vec<PropertyList>> {
        let m = self.save.model("MercCompanyModel").ok()?;
        let fsl = get_path_mut(&mut m.props, &["FactionStandingList", "Standings"])?;
        match &mut fsl.value {
            Value::Array { body: ArrayBody::Structs { elements, .. }, .. } => Some(elements),
            _ => None,
        }
    }

    pub fn factions(&mut self) -> Vec<FactionInfo> {
        let Some(st) = self.standings() else { return Vec::new() };
        st.iter().enumerate().map(|(i, el)| FactionInfo {
            index: i,
            name: get_path(el, &["Faction", "ID", "PrimaryAssetName"]).and_then(|p| p.as_fstring()).unwrap_or_default(),
            standing: el.get("Standing").and_then(|p| p.as_i32()).unwrap_or(0),
        }).collect()
    }

    pub fn set_faction_standing(&mut self, index: usize, v: i32) -> bool {
        let Some(st) = self.standings() else { return false };
        let Some(el) = st.get_mut(index) else { return false };
        el.get_mut("Standing").map(|p| set_scalar(p, v.to_le_bytes().to_vec())).unwrap_or(false)
    }
}
