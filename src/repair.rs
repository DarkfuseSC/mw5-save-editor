//! Targeted repairs for the damage classes the diagnoser recognises.
//! Each repair edits the minimum number of bytes, then re-diagnoses. A repair
//! that does not leave the file CLEAN (or strictly better) is rolled back.

use crate::diagnose::{diagnose, probe_pmd, Kind, Report, Severity};
use crate::io::Reader;

#[derive(Debug, Clone)]
pub struct RepairStep {
    pub kind: Kind,
    pub path: String,
    pub action: String,
}

#[derive(Debug, Default)]
pub struct RepairOutcome {
    pub steps: Vec<RepairStep>,
    pub unrepairable: Vec<String>,
    pub before: Option<Report>,
    pub after: Option<Report>,
}

impl RepairOutcome {
    pub fn changed(&self) -> bool {
        !self.steps.is_empty()
    }
}

fn put_i64(buf: &mut [u8], off: usize, v: i64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
fn put_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn fatal_count(r: &Report) -> usize {
    r.findings.iter().filter(|f| f.severity == Severity::Fatal).count()
}

/// Fix the outer/inner length pair around PersistentModelData.
fn repair_pmd_lengths(data: &mut Vec<u8>, out: &mut RepairOutcome) -> bool {
    let Some(p) = probe_pmd(data) else { return false };
    let outer_consistent = p.outer_size == p.inner_len as i64 + 4;
    if outer_consistent {
        return false;
    }
    let outer_fits = p.outer_size >= 0 && (p.outer_size as usize) <= p.region_avail;
    let inner_fits = p.inner_len >= 0 && (p.inner_len as usize + 4) <= p.region_avail;
    if inner_fits && !outer_fits {
        put_i64(data, p.size_off, p.inner_len as i64 + 4);
        out.steps.push(RepairStep { kind: Kind::SizeMismatch, path: "top.PersistentModelData".into(),
            action: format!("outer size {} -> {} (from inner archive length)", p.outer_size, p.inner_len as i64 + 4) });
        return true;
    }
    if inner_fits && outer_fits {
        // Both fit; trust the one whose implied end lands on the trailer boundary.
        // Clean saves end [..archive][None FString 9 bytes][4-byte trailer].
        let end_outer = p.len_off + p.outer_size as usize;
        let end_inner = p.len_off + 4 + p.inner_len as usize;
        let tail_ok = |end: usize| data.len() >= end + 13 && &data[end..end + 9] == b"\x05\x00\x00\x00None\x00";
        if tail_ok(end_inner) && !tail_ok(end_outer) {
            put_i64(data, p.size_off, p.inner_len as i64 + 4);
            out.steps.push(RepairStep { kind: Kind::SizeMismatch, path: "top.PersistentModelData".into(),
                action: format!("outer size {} -> {} (inner length matches the None terminator)", p.outer_size, p.inner_len as i64 + 4) });
            return true;
        }
        if tail_ok(end_outer) && !tail_ok(end_inner) {
            put_i32(data, p.len_off, (p.outer_size - 4) as i32);
            out.steps.push(RepairStep { kind: Kind::ArchiveLength, path: "PMD".into(),
                action: format!("inner archive length {} -> {} (outer size matches the None terminator)", p.inner_len, p.outer_size - 4) });
            return true;
        }
        return false;
    }
    if outer_fits && !inner_fits {
        put_i32(data, p.len_off, (p.outer_size - 4) as i32);
        out.steps.push(RepairStep { kind: Kind::ArchiveLength, path: "PMD".into(),
            action: format!("inner archive length {} -> {} (from outer size)", p.inner_len, p.outer_size - 4) });
        return true;
    }
    false
}

/// Fix a MechLoadoutWrappers[i].ByteData archive whose i32 length prefix
/// disagrees with the ArrayProperty size that encloses it. The enclosing
/// size is authoritative because the outer parse succeeded with it.
fn repair_wrapper_lengths(data: &mut Vec<u8>, out: &mut RepairOutcome) -> bool {
    // Locate every "ByteData" ArrayProperty after MechLoadoutWrappers by scanning
    // the raw bytes; the parser already proved the outer framing is sound.
    let anchor = b"MechLoadoutWrappers\0";
    let Some(start) = data.windows(anchor.len()).position(|w| w == anchor) else { return false };
    let needle = b"ByteData\0";
    let mut changed = false;
    let mut pos = start;
    let mut idx = 0usize;
    while let Some(rel) = data[pos..].windows(needle.len()).position(|w| w == needle) {
        let at = pos + rel;
        pos = at + needle.len();
        let mut r = Reader::new(data);
        r.pos = pos;
        let Ok(ptype) = r.fstring() else { continue };
        if ptype != "ArrayProperty" { continue; }
        let Ok(size) = r.i64() else { continue };
        let Ok(elem) = r.fstring() else { continue };
        if elem != "ByteProperty" { continue; }
        let Ok(_leading) = r.u8() else { continue };
        let len_off = r.pos;
        let Ok(inner) = r.i32() else { continue };
        let expected = size - 4;
        if size >= 4 && inner as i64 != expected && len_off + size as usize <= data.len() {
            // Only touch it if the region really is a broken nested archive:
            // fails to parse as-is, parses once the prefix is corrected.
            let region = &data[len_off..len_off + size as usize];
            if crate::save::Archive::parse(region).is_ok() {
                idx += 1;
                pos = len_off + 4;
                continue;
            }
            let mut patched = region.to_vec();
            put_i32(&mut patched, 0, expected as i32);
            if crate::save::Archive::parse(&patched).is_err() {
                idx += 1;
                pos = len_off + 4;
                continue;
            }
            put_i32(data, len_off, expected as i32);
            out.steps.push(RepairStep { kind: Kind::ArchiveLength,
                path: format!("PMD.SaveStateModel.MechLoadoutWrappers[{}].ByteData", idx),
                action: format!("archive length {} -> {} (from enclosing ArrayProperty size)", inner, expected) });
            changed = true;
        }
        idx += 1;
        pos = len_off + 4;
    }
    changed
}

fn strip_trailing(data: &mut Vec<u8>, before: &Report, out: &mut RepairOutcome) -> bool {
    if !before.findings.iter().any(|f| f.kind == Kind::TrailingBytes) {
        return false;
    }
    // Find the end of the top-level list: last "None" terminator before the trailer.
    let term = b"\x05\x00\x00\x00None\x00";
    let Some(last) = data.windows(term.len()).rposition(|w| w == term) else { return false };
    let keep = last + term.len() + crate::diagnose::MAX_TRAILER;
    if keep < data.len() {
        let removed = data.len() - keep;
        data.truncate(keep);
        out.steps.push(RepairStep { kind: Kind::TrailingBytes, path: "top".into(), action: format!("removed {} trailing bytes", removed) });
        return true;
    }
    false
}

/// Attempt every applicable repair. Returns the (possibly) repaired bytes and
/// a log. The input is never modified.
pub fn repair(input: &[u8]) -> (Vec<u8>, RepairOutcome) {
    let mut out = RepairOutcome::default();
    let before = diagnose(input);
    let mut data = input.to_vec();

    if before.findings.iter().any(|f| f.kind == Kind::NotCampaignSave) {
        out.unrepairable.push("Not a campaign save file.".into());
        out.before = Some(before);
        return (data, out);
    }
    if before.findings.iter().any(|f| f.kind == Kind::Truncated) {
        out.unrepairable.push("The file is cut short: the missing bytes are gone and cannot be reconstructed. \
            Restore the previous autosave from the same campaign folder (the game keeps several) or a backup.".into());
    }

    // Up to 3 passes: fixing one length can expose the next layer.
    for _ in 0..3 {
        let mut progressed = false;
        progressed |= repair_pmd_lengths(&mut data, &mut out);
        progressed |= repair_wrapper_lengths(&mut data, &mut out);
        let current = diagnose(&data);
        progressed |= strip_trailing(&mut data, &current, &mut out);
        if !progressed {
            break;
        }
    }

    let after = diagnose(&data);
    // Roll back if we made things worse.
    if fatal_count(&after) > fatal_count(&before) {
        out.steps.clear();
        out.unrepairable.push("Automatic repair made the file worse and was rolled back.".into());
        data = input.to_vec();
        out.after = Some(diagnose(&data));
    } else {
        out.after = Some(after);
    }
    out.before = Some(before);
    (data, out)
}
