//! UE4 tagged-property list as used by MW5 Mercs .sav files.
//!
//! Lossless by construction: every byte that is not understood is kept as a
//! raw blob and written back verbatim. Sizes are always recomputed on write.
//! Format rules are documented in knowledgebase.md.

use crate::io::{Reader, Result, Writer};

pub const NONE: &str = "None";

/// Struct types serialized as fixed-layout raw values rather than nested
/// tagged-property lists.
pub const RAW_STRUCTS: &[&str] = &[
    "Vector", "Vector2D", "Rotator", "Quat", "Guid", "DateTime", "Timespan",
    "Color", "LinearColor", "Transform", "Box", "Box2D",
    // Engine struct with custom serializer (tag name array); 462 occurrences
    // in a clean save, all under ContractModel / RosterModel / SaveStateModel.
    "GameplayTagContainer",
];

#[derive(Clone, Debug)]
pub struct Property {
    pub name: String,
    pub ptype: String,
    pub value: Value,
}

#[derive(Clone, Debug, Default)]
pub struct PropertyList {
    pub props: Vec<Property>,
}

#[derive(Clone, Debug)]
pub enum Value {
    Struct {
        struct_name: String,
        guid: [u8; 16],
        leading: u8,
        body: StructBody,
    },
    Array {
        elem_type: String,
        leading: u8,
        body: ArrayBody,
    },
    /// Map/Set: sub-types kept, content opaque (includes its leading byte).
    MapSet {
        sub_types: Vec<String>,
        blob: Vec<u8>,
    },
    Bool {
        value: u8,
        trailing: u8,
    },
    /// EnumProperty and scalar ByteProperty: enum name + trailing + payload.
    Enum {
        enum_name: String,
        trailing: u8,
        payload: Vec<u8>,
    },
    Scalar {
        trailing: u8,
        payload: Vec<u8>,
    },
}

#[derive(Clone, Debug)]
pub enum StructBody {
    Tagged(PropertyList),
    Raw(Vec<u8>),
}

#[derive(Clone, Debug)]
pub enum ArrayBody {
    /// StructProperty arrays: one inner tag, then N tagged lists.
    Structs {
        inner_name: String,
        inner_struct: String,
        inner_guid: [u8; 16],
        inner_leading: u8,
        elements: Vec<PropertyList>,
    },
    /// Everything else (and undecodable struct arrays): count + elements verbatim.
    Raw(Vec<u8>),
}

impl PropertyList {
    pub fn get(&self, name: &str) -> Option<&Property> {
        self.props.iter().find(|p| p.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Property> {
        self.props.iter_mut().find(|p| p.name == name)
    }

    /// Parse a list from `data`, requiring the whole slice to be consumed.
    pub fn parse_exact(data: &[u8]) -> Result<PropertyList> {
        let mut r = Reader::new(data);
        let pl = read_list(&mut r)?;
        if r.remaining() != 0 {
            return Err(crate::io::ParseError {
                pos: r.pos,
                msg: format!("{} trailing bytes after None terminator", r.remaining()),
            });
        }
        Ok(pl)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        write_list(&mut w, self);
        w.into_bytes()
    }
}

fn guid16(b: &[u8]) -> [u8; 16] {
    let mut g = [0u8; 16];
    g.copy_from_slice(b);
    g
}

pub fn read_list(r: &mut Reader) -> Result<PropertyList> {
    let mut props = Vec::new();
    loop {
        let name = r.fstring()?;
        if name == NONE {
            break;
        }
        let ptype = r.fstring()?;
        let value = read_value(r, &ptype)?;
        props.push(Property { name, ptype, value });
    }
    Ok(PropertyList { props })
}

/// Read one property value (everything after the name and type FStrings).
pub fn read_list_one_value(r: &mut Reader, ptype: &str) -> Result<Value> {
    read_value(r, ptype)
}

fn read_value(r: &mut Reader, ptype: &str) -> Result<Value> {
    let size = r.i64()?;
    match ptype {
        "StructProperty" => {
            let struct_name = r.fstring()?;
            let guid = guid16(r.read(16)?);
            let leading = r.u8()?;
            let content = r.sized(size)?;
            let body = if RAW_STRUCTS.contains(&struct_name.as_str()) {
                StructBody::Raw(content.to_vec())
            } else {
                match PropertyList::parse_exact(content) {
                    Ok(pl) => StructBody::Tagged(pl),
                    Err(_) => StructBody::Raw(content.to_vec()),
                }
            };
            Ok(Value::Struct { struct_name, guid, leading, body })
        }
        "ArrayProperty" => {
            let elem_type = r.fstring()?;
            let leading = r.u8()?;
            let payload = r.sized(size)?;
            let body = if elem_type == "StructProperty" {
                decode_struct_array(payload).unwrap_or_else(|| ArrayBody::Raw(payload.to_vec()))
            } else {
                ArrayBody::Raw(payload.to_vec())
            };
            Ok(Value::Array { elem_type, leading, body })
        }
        "MapProperty" | "SetProperty" => {
            let mut sub_types = vec![r.fstring()?];
            if ptype == "MapProperty" {
                sub_types.push(r.fstring()?);
            }
            let blob = r.sized(size + 1)?.to_vec();
            Ok(Value::MapSet { sub_types, blob })
        }
        "BoolProperty" => {
            let value = r.u8()?;
            let trailing = r.u8()?;
            Ok(Value::Bool { value, trailing })
        }
        "EnumProperty" | "ByteProperty" => {
            let enum_name = r.fstring()?;
            let trailing = r.u8()?;
            let payload = r.sized(size)?.to_vec();
            Ok(Value::Enum { enum_name, trailing, payload })
        }
        _ => {
            let trailing = r.u8()?;
            let payload = r.sized(size)?.to_vec();
            Ok(Value::Scalar { trailing, payload })
        }
    }
}

fn decode_struct_array(payload: &[u8]) -> Option<ArrayBody> {
    let mut r = Reader::new(payload);
    let count = r.i32().ok()?;
    if count < 0 {
        return None;
    }
    let inner_name = r.fstring().ok()?;
    let inner_type = r.fstring().ok()?;
    if inner_type != "StructProperty" {
        return None;
    }
    let _inner_size = r.i64().ok()?;
    let inner_struct = r.fstring().ok()?;
    let inner_guid = guid16(r.read(16).ok()?);
    let inner_leading = r.u8().ok()?;
    let mut elements = Vec::with_capacity(count as usize);
    for _ in 0..count {
        elements.push(read_list(&mut r).ok()?);
    }
    if r.remaining() != 0 {
        return None;
    }
    Some(ArrayBody::Structs { inner_name, inner_struct, inner_guid, inner_leading, elements })
}

pub fn write_list(w: &mut Writer, pl: &PropertyList) {
    for p in &pl.props {
        w.fstring(&p.name);
        w.fstring(&p.ptype);
        write_value(w, &p.value);
    }
    w.fstring(NONE);
}

fn write_value(w: &mut Writer, v: &Value) {
    match v {
        Value::Struct { struct_name, guid, leading, body } => {
            let content = match body {
                StructBody::Tagged(pl) => pl.to_bytes(),
                StructBody::Raw(b) => b.clone(),
            };
            w.i64(content.len() as i64);
            w.fstring(struct_name);
            w.write(guid);
            w.u8(*leading);
            w.write(&content);
        }
        Value::Array { elem_type, leading, body } => {
            let payload = match body {
                ArrayBody::Raw(b) => b.clone(),
                ArrayBody::Structs { inner_name, inner_struct, inner_guid, inner_leading, elements } => {
                    let mut ew = Writer::new();
                    for e in elements {
                        write_list(&mut ew, e);
                    }
                    let eb = ew.into_bytes();
                    let mut pw = Writer::new();
                    pw.i32(elements.len() as i32);
                    pw.fstring(inner_name);
                    pw.fstring("StructProperty");
                    pw.i64(eb.len() as i64);
                    pw.fstring(inner_struct);
                    pw.write(inner_guid);
                    pw.u8(*inner_leading);
                    pw.write(&eb);
                    pw.into_bytes()
                }
            };
            w.i64(payload.len() as i64);
            w.fstring(elem_type);
            w.u8(*leading);
            w.write(&payload);
        }
        Value::MapSet { sub_types, blob } => {
            w.i64(blob.len() as i64 - 1);
            for s in sub_types {
                w.fstring(s);
            }
            w.write(blob);
        }
        Value::Bool { value, trailing } => {
            w.i64(0);
            w.u8(*value);
            w.u8(*trailing);
        }
        Value::Enum { enum_name, trailing, payload } => {
            w.i64(payload.len() as i64);
            w.fstring(enum_name);
            w.u8(*trailing);
            w.write(payload);
        }
        Value::Scalar { trailing, payload } => {
            w.i64(payload.len() as i64);
            w.u8(*trailing);
            w.write(payload);
        }
    }
}

// Convenience accessors for scalar payloads.
impl Property {
    pub fn payload(&self) -> Option<&[u8]> {
        match &self.value {
            Value::Scalar { payload, .. } | Value::Enum { payload, .. } => Some(payload),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.payload().filter(|p| p.len() == 4).map(|p| i32::from_le_bytes(p.try_into().unwrap()))
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.payload().filter(|p| p.len() == 8).map(|p| i64::from_le_bytes(p.try_into().unwrap()))
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.payload().filter(|p| p.len() == 4).map(|p| f32::from_le_bytes(p.try_into().unwrap()))
    }

    pub fn as_fstring(&self) -> Option<String> {
        let mut r = Reader::new(self.payload()?);
        r.fstring().ok()
    }

    pub fn as_struct(&self) -> Option<&PropertyList> {
        match &self.value {
            Value::Struct { body: StructBody::Tagged(pl), .. } => Some(pl),
            _ => None,
        }
    }

    pub fn as_struct_mut(&mut self) -> Option<&mut PropertyList> {
        match &mut self.value {
            Value::Struct { body: StructBody::Tagged(pl), .. } => Some(pl),
            _ => None,
        }
    }

    pub fn as_struct_array(&self) -> Option<&Vec<PropertyList>> {
        match &self.value {
            Value::Array { body: ArrayBody::Structs { elements, .. }, .. } => Some(elements),
            _ => None,
        }
    }
}
