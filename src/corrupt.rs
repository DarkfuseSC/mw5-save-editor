//! Deterministic corruption generators for test fixtures. Each takes a
//! known-good save and produces one damage class, so the diagnoser and the
//! repair layer can be tested without waiting for real victims.

use crate::io::Reader;

pub const KINDS: &[&str] = &[
    "truncate-mid", "truncate-tail", "size-inflate", "size-shrink",
    "pmd-length", "wrapper-length", "garbage-tail",
];

/// Offset of the i64 size field of the first top-level property named `name`.
fn find_top_size_offset(data: &[u8], name: &str) -> Option<usize> {
    let mut r = Reader::new(data);
    loop {
        let n = r.fstring().ok()?;
        if n == "None" {
            return None;
        }
        let ptype = r.fstring().ok()?;
        let size_off = r.pos;
        if n == name {
            return Some(size_off);
        }
        // skip this property using the real parser
        crate::prop::read_list_one_value(&mut r, &ptype).ok()?;
    }
}

fn put_i64(buf: &mut [u8], off: usize, v: i64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}
fn get_i64(buf: &[u8], off: usize) -> i64 {
    i64::from_le_bytes(buf[off..off + 8].try_into().unwrap())
}
fn put_i32(buf: &mut [u8], off: usize, v: i32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

pub fn apply(kind: &str, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = data.to_vec();
    match kind {
        "truncate-mid" => {
            out.truncate(data.len() / 2);
        }
        "truncate-tail" => {
            out.truncate(data.len().saturating_sub(37));
        }
        "size-inflate" | "size-shrink" => {
            // Pick the first top-level StructProperty/ArrayProperty after the
            // opening ones; corrupt its size by a few hundred bytes.
            let off = find_top_size_offset(data, "PersistentModelData")
                .ok_or("PersistentModelData not found")?;
            let cur = get_i64(data, off);
            let delta = if kind == "size-inflate" { 512 } else { -512 };
            put_i64(&mut out, off, cur + delta);
        }
        "pmd-length" => {
            // The nested archive's i32 length prefix sits right after the
            // ArrayProperty tag: size(8) + "ByteProperty" FString + leading byte.
            let off = find_top_size_offset(data, "PersistentModelData")
                .ok_or("PersistentModelData not found")?;
            let mut r = Reader::new(data);
            r.pos = off;
            let _size = r.i64().map_err(|e| e.to_string())?;
            let _elem = r.fstring().map_err(|e| e.to_string())?;
            let _leading = r.u8().map_err(|e| e.to_string())?;
            let len_off = r.pos;
            let cur = i32::from_le_bytes(data[len_off..len_off + 4].try_into().unwrap());
            put_i32(&mut out, len_off, cur + 99);
        }
        "wrapper-length" => {
            // Find the first "ByteData" ArrayProperty<ByteProperty> anywhere
            // and bump its nested archive length prefix.
            let anchor = b"MechLoadoutWrappers\0";
            let start = data.windows(anchor.len()).position(|w| w == anchor).ok_or("no MechLoadoutWrappers found")?;
            let needle = b"ByteData\0";
            let pos = start + data[start..].windows(needle.len()).position(|w| w == needle).ok_or("no ByteData after MechLoadoutWrappers")?;
            let mut r = Reader::new(data);
            r.pos = pos + needle.len();
            let ptype = r.fstring().map_err(|e| e.to_string())?;
            if ptype != "ArrayProperty" {
                return Err(format!("ByteData is {} not ArrayProperty", ptype));
            }
            let _size = r.i64().map_err(|e| e.to_string())?;
            let _elem = r.fstring().map_err(|e| e.to_string())?;
            let _leading = r.u8().map_err(|e| e.to_string())?;
            let len_off = r.pos;
            let cur = i32::from_le_bytes(data[len_off..len_off + 4].try_into().unwrap());
            put_i32(&mut out, len_off, cur - 7);
        }
        "garbage-tail" => {
            out.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0, 0, 0, 0, 1, 2, 3]);
        }
        _ => return Err(format!("unknown kind {}", kind)),
    }
    Ok(out)
}
