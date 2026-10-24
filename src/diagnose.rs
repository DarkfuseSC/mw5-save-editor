//! Corruption detection. Classifies damage so the repair layer can pick a
//! targeted fix instead of a blind rewrite.
//!
//! Levels checked, outermost first:
//!   1. top-level tagged list (hard parse)
//!   2. PersistentModelData archive length + list
//!   3. each model (ObjectProperty payload)
//!   4. each MechLoadoutWrapper.ByteData archive
//! plus a tree walk that flags silent fallbacks: a non-raw struct that
//! failed to parse as a tagged list, or a StructProperty array that could
//! not be decoded. Those never fail the parser (lossless rule) but they are
//! exactly where a size-field corruption inside a save shows up.

use crate::io::{ParseError, Reader};
use crate::prop::{read_list, ArrayBody, PropertyList, StructBody, Value, RAW_STRUCTS};
use crate::save::{looks_like_campaign_save, Archive, Model, SaveFile};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Severity {
    /// File cannot be loaded by the game and cannot be edited as-is.
    Fatal,
    /// Parsed, but a sub-structure is opaque where it should be structured.
    Suspect,
    Info,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    NotCampaignSave,
    /// Data ends before a declared length is satisfied (crash mid-write).
    Truncated,
    /// A size/length field disagrees with the bytes that follow it.
    SizeMismatch,
    /// Nested archive (PMD / wrapper ByteData) has a bad length prefix.
    ArchiveLength,
    /// Tagged struct that did not parse as a property list.
    OpaqueStruct,
    /// StructProperty array that did not decode.
    OpaqueStructArray,
    /// Required top-level model missing.
    MissingModel,
    /// Unexpected bytes after the top-level None terminator.
    TrailingBytes,
    Ok,
}

/// Clean saves on the dev box carry a 4-byte trailer after the top list.
pub const MAX_TRAILER: usize = 4;

#[derive(Clone, Debug)]
pub struct Finding {
    pub severity: Severity,
    pub kind: Kind,
    /// Dotted path into the save, e.g. `PMD.SaveStateModel.MechLoadoutWrappers[12].ByteData`.
    pub path: String,
    pub detail: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} {:?} {} : {}", self.severity, self.kind, self.path, self.detail)
    }
}

#[derive(Default, Debug)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub models: usize,
    pub wrappers: usize,
    pub trailer_len: usize,
}

impl Report {
    pub fn fatal(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Fatal)
    }
    pub fn suspect(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Suspect)
    }
    fn push(&mut self, severity: Severity, kind: Kind, path: impl Into<String>, detail: impl Into<String>) {
        self.findings.push(Finding { severity, kind, path: path.into(), detail: detail.into() });
    }
}

/// Classify a hard parse error by its message and where it stopped relative
/// to the end of the data it was reading.
fn classify_parse_error(e: &ParseError, total_len: usize) -> Kind {
    if e.msg.starts_with("wanted") && e.msg.contains("left") {
        // Reader ran out of bytes. Near EOF of the outer file => truncated;
        // otherwise a length field pointed past the region it lives in.
        if e.pos + 64 >= total_len {
            Kind::Truncated
        } else {
            Kind::SizeMismatch
        }
    } else if e.msg.contains("declared size") || e.msg.contains("negative size") || e.msg.contains("trailing bytes") {
        Kind::SizeMismatch
    } else if e.msg.contains("NUL") {
        Kind::SizeMismatch
    } else {
        Kind::SizeMismatch
    }
}

/// What the two length fields around PersistentModelData say. Lets a top-level
/// parse failure be split into "file is cut short" versus "outer size field is
/// wrong", which need different repairs.
#[derive(Debug, Clone, Copy)]
pub struct PmdProbe {
    /// Offset of the outer i64 size field.
    pub size_off: usize,
    pub outer_size: i64,
    /// Offset of the inner i32 archive length.
    pub len_off: usize,
    pub inner_len: i32,
    /// Bytes from the start of the archive region to end of file.
    pub region_avail: usize,
}

pub fn probe_pmd(data: &[u8]) -> Option<PmdProbe> {
    let mut r = Reader::new(data);
    loop {
        let name = r.fstring().ok()?;
        if name == "None" {
            return None;
        }
        let ptype = r.fstring().ok()?;
        if name == "PersistentModelData" {
            if ptype != "ArrayProperty" {
                return None;
            }
            let size_off = r.pos;
            let outer_size = r.i64().ok()?;
            let _elem = r.fstring().ok()?;
            let _leading = r.u8().ok()?;
            let len_off = r.pos;
            let inner_len = r.i32().ok()?;
            return Some(PmdProbe { size_off, outer_size, len_off, inner_len, region_avail: data.len() - len_off });
        }
        // Earlier properties are small scalars; skip with the real parser.
        crate::prop::read_list_one_value(&mut r, &ptype).ok()?;
    }
}

fn refine_top_failure(data: &[u8], e: &ParseError) -> (Kind, String) {
    let base = classify_parse_error(e, data.len());
    let Some(p) = probe_pmd(data) else { return (base, String::new()) };
    let outer_consistent = p.outer_size == p.inner_len as i64 + 4;
    let outer_fits = p.outer_size >= 0 && (p.outer_size as usize) <= p.region_avail;
    let inner_fits = p.inner_len >= 0 && (p.inner_len as usize + 4) <= p.region_avail;
    let note = format!("PMD outer size {} @{:#x}, inner len {} @{:#x}, {} bytes available",
        p.outer_size, p.size_off, p.inner_len, p.len_off, p.region_avail);
    if outer_consistent && !outer_fits {
        return (Kind::Truncated, format!("{}; both lengths agree and exceed the file: file is cut short by {} bytes", note, p.outer_size as usize - p.region_avail));
    }
    if !outer_consistent && inner_fits {
        return (Kind::SizeMismatch, format!("{}; inner archive length is plausible, outer size field is wrong (repairable: set outer = inner + 4)", note));
    }
    if !outer_consistent && outer_fits {
        return (Kind::ArchiveLength, format!("{}; outer size fits, inner archive length is wrong (repairable: set inner = outer - 4)", note));
    }
    (base, note)
}

/// Archive::parse fails at offset 4 when the i32 length overruns, and with
/// "bytes after nested archive" when it underruns. Either way the length
/// prefix is the broken field. Deeper errors are inner content damage.
fn classify_archive_error(e: &ParseError, region_len: usize) -> Kind {
    if e.pos == 4 || e.msg.contains("after nested archive") {
        Kind::ArchiveLength
    } else {
        classify_parse_error(e, region_len)
    }
}

pub const REQUIRED_MODELS: &[&str] = &[
    "SaveStateModel", "MechStorageModel", "TimelineModel", "InventoryModel",
];

pub fn diagnose(data: &[u8]) -> Report {
    let mut rep = Report::default();

    if !looks_like_campaign_save(data) {
        rep.push(Severity::Fatal, Kind::NotCampaignSave, "", "first bytes are not a tagged-property FString");
        return rep;
    }

    // 1. top level
    let mut r = Reader::new(data);
    let top = match read_list(&mut r) {
        Ok(t) => t,
        Err(e) => {
            let (kind, note) = refine_top_failure(data, &e);
            rep.push(Severity::Fatal, kind, "top", format!("{} (file len {}){}{}", e, data.len(), if note.is_empty() { "" } else { "; " }, note));
            return rep;
        }
    };
    walk_list(&top, "top", &mut rep);
    let trailer = data.len() - r.pos;
    rep.trailer_len = trailer;
    if trailer > MAX_TRAILER {
        rep.push(Severity::Suspect, Kind::TrailingBytes, "top",
            format!("{} bytes after the top-level None terminator (expected <= {})", trailer, MAX_TRAILER));
    }

    // 2. PMD archive
    let pmd_prop = match top.get("PersistentModelData") {
        Some(p) => p,
        None => {
            rep.push(Severity::Fatal, Kind::MissingModel, "top", "PersistentModelData not present");
            return rep;
        }
    };
    let pmd_bytes = match &pmd_prop.value {
        Value::Array { body: ArrayBody::Raw(b), .. } => b,
        _ => {
            rep.push(Severity::Fatal, Kind::SizeMismatch, "top.PersistentModelData", "unexpected shape");
            return rep;
        }
    };
    let pmd = match Archive::parse(pmd_bytes) {
        Ok(a) => a,
        Err(e) => {
            let kind = classify_archive_error(&e, pmd_bytes.len());
            rep.push(Severity::Fatal, kind, "PMD", format!("{} (archive region {} bytes)", e, pmd_bytes.len()));
            return rep;
        }
    };

    // 3. models
    for p in &pmd.props.props {
        let path = format!("PMD.{}", p.name);
        let payload = match p.payload() {
            Some(b) => b,
            None => {
                rep.push(Severity::Suspect, Kind::SizeMismatch, &path, format!("model entry is {} not ObjectProperty", p.ptype));
                continue;
            }
        };
        match Model::parse(payload) {
            Ok(m) if m.is_reference => {}
            Ok(m) => {
                rep.models += 1;
                walk_list(&m.props, &path, &mut rep);
                if p.name == "SaveStateModel" {
                    check_wrappers(&m.props, &path, &mut rep);
                }
            }
            Err(e) => {
                let kind = classify_parse_error(&e, payload.len());
                rep.push(Severity::Fatal, kind, &path, e.to_string());
            }
        }
    }
    for req in REQUIRED_MODELS {
        if pmd.props.get(req).is_none() {
            rep.push(Severity::Fatal, Kind::MissingModel, format!("PMD.{}", req), "required model missing");
        }
    }

    if rep.findings.is_empty() {
        rep.push(Severity::Info, Kind::Ok, "", format!("{} models, {} mech wrappers, trailer {} bytes, no damage found", rep.models, rep.wrappers, rep.trailer_len));
    }
    rep
}

fn check_wrappers(props: &PropertyList, path: &str, rep: &mut Report) {
    let Some(w) = props.get("MechLoadoutWrappers") else {
        rep.push(Severity::Suspect, Kind::MissingModel, format!("{}.MechLoadoutWrappers", path), "array missing");
        return;
    };
    let Value::Array { body: ArrayBody::Structs { elements, .. }, .. } = &w.value else {
        rep.push(Severity::Suspect, Kind::OpaqueStructArray, format!("{}.MechLoadoutWrappers", path), "array did not decode");
        return;
    };
    for (i, el) in elements.iter().enumerate() {
        let wpath = format!("{}.MechLoadoutWrappers[{}].ByteData", path, i);
        let Some(bd) = el.get("ByteData") else { continue };
        let Value::Array { body: ArrayBody::Raw(b), .. } = &bd.value else { continue };
        match Archive::parse(b) {
            Ok(a) => {
                rep.wrappers += 1;
                walk_list(&a.props, &wpath, rep);
            }
            Err(e) => {
                let kind = classify_archive_error(&e, b.len());
                // Game can usually still load with one bad wrapper; the mech is lost.
                rep.push(Severity::Fatal, kind, &wpath, e.to_string());
            }
        }
    }
}

/// Flag silent parser fallbacks.
fn walk_list(pl: &PropertyList, path: &str, rep: &mut Report) {
    for p in &pl.props {
        let ppath = format!("{}.{}", path, p.name);
        match &p.value {
            Value::Struct { struct_name, body: StructBody::Raw(b), .. } => {
                if !RAW_STRUCTS.contains(&struct_name.as_str()) {
                    rep.push(Severity::Suspect, Kind::OpaqueStruct, &ppath,
                        format!("struct {} ({} bytes) is not a valid tagged list", struct_name, b.len()));
                }
            }
            Value::Struct { body: StructBody::Tagged(sub), .. } => walk_list(sub, &ppath, rep),
            Value::Array { elem_type, body: ArrayBody::Raw(b), .. } if elem_type == "StructProperty" => {
                rep.push(Severity::Suspect, Kind::OpaqueStructArray, &ppath,
                    format!("struct array ({} bytes) did not decode", b.len()));
            }
            Value::Array { body: ArrayBody::Structs { elements, .. }, .. } => {
                for (i, el) in elements.iter().enumerate() {
                    walk_list(el, &format!("{}[{}]", ppath, i), rep);
                }
            }
            _ => {}
        }
    }
}

/// Convenience: diagnose then, if clean, parse fully.
pub fn open_checked(data: &[u8]) -> (Report, Option<SaveFile>) {
    let rep = diagnose(data);
    if rep.fatal() {
        return (rep, None);
    }
    (rep, SaveFile::parse(data).ok())
}
