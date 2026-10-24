//! MW5 save file: top-level list -> PersistentModelData archive -> models.

use crate::io::{ParseError, Reader, Result, Writer};
use crate::prop::{read_list, write_list, ArrayBody, PropertyList, Value};
use std::collections::BTreeMap;

/// A gameplay model object: ObjectProperty payload = [class path][list][footer].
#[derive(Clone, Debug)]
pub struct Model {
    pub class_path: String,
    pub props: PropertyList,
    pub footer: Vec<u8>,
    /// True when the ObjectProperty payload is only an object path (e.g. the
    /// `Owner` back-reference some saves carry in PersistentModelData) and
    /// has no property list after it. Such entries are references, not models.
    pub is_reference: bool,
}

impl Model {
    pub fn parse(payload: &[u8]) -> Result<Model> {
        let mut r = Reader::new(payload);
        let class_path = r.fstring()?;
        if r.remaining() == 0 {
            return Ok(Model { class_path, props: PropertyList::default(), footer: Vec::new(), is_reference: true });
        }
        let props = read_list(&mut r)?;
        let footer = payload[r.pos..].to_vec();
        Ok(Model { class_path, props, footer, is_reference: false })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.fstring(&self.class_path);
        if self.is_reference {
            return w.into_bytes();
        }
        write_list(&mut w, &self.props);
        w.write(&self.footer);
        w.into_bytes()
    }
}

/// An i32-length-prefixed nested archive holding a property list plus footer
/// (PersistentModelData payload, MechLoadoutWrapper.ByteData payload).
#[derive(Clone, Debug)]
pub struct Archive {
    pub props: PropertyList,
    pub footer: Vec<u8>,
}

impl Archive {
    pub fn parse(payload: &[u8]) -> Result<Archive> {
        let mut r = Reader::new(payload);
        let len = r.i32()?;
        let arc = r.sized(len as i64)?;
        if r.remaining() != 0 {
            return Err(ParseError { pos: r.pos, msg: "bytes after nested archive".into() });
        }
        let mut ar = Reader::new(arc);
        let props = read_list(&mut ar)?;
        let footer = arc[ar.pos..].to_vec();
        Ok(Archive { props, footer })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut inner = Writer::new();
        write_list(&mut inner, &self.props);
        inner.write(&self.footer);
        let inner = inner.into_bytes();
        let mut w = Writer::new();
        w.i32(inner.len() as i32);
        w.write(&inner);
        w.into_bytes()
    }
}

/// Cheap sniff: a campaign save starts with a tagged-property list, so the
/// first 4 bytes are a small positive FString length followed by an ASCII
/// property name and NUL. Files like `saved_contacts_*.sav` or Instant
/// Action deployment blobs fail this and are a different format.
pub fn looks_like_campaign_save(data: &[u8]) -> bool {
    if data.len() < 8 {
        return false;
    }
    let n = i32::from_le_bytes(data[0..4].try_into().unwrap());
    if n <= 1 || n > 256 || data.len() < 4 + n as usize {
        return false;
    }
    let s = &data[4..4 + n as usize];
    s[s.len() - 1] == 0 && s[..s.len() - 1].iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

pub struct SaveFile {
    pub top: PropertyList,
    pub trailer: Vec<u8>,
    /// Parsed PersistentModelData archive (list of ObjectProperty models).
    pub pmd: Archive,
    /// Models parsed on demand; written back on `to_bytes`.
    models: BTreeMap<String, Model>,
}

impl SaveFile {
    pub fn parse(data: &[u8]) -> Result<SaveFile> {
        let mut r = Reader::new(data);
        let top = read_list(&mut r)?;
        let trailer = data[r.pos..].to_vec();
        let pmd_prop = top.get("PersistentModelData").ok_or(ParseError {
            pos: 0,
            msg: "PersistentModelData not found".into(),
        })?;
        let pmd = match &pmd_prop.value {
            Value::Array { body: ArrayBody::Raw(b), .. } => Archive::parse(b)?,
            _ => {
                return Err(ParseError { pos: 0, msg: "PersistentModelData has unexpected shape".into() })
            }
        };
        Ok(SaveFile { top, trailer, pmd, models: BTreeMap::new() })
    }

    pub fn model_names(&self) -> Vec<String> {
        self.pmd.props.props.iter().map(|p| p.name.clone()).collect()
    }

    pub fn model(&mut self, name: &str) -> Result<&mut Model> {
        if !self.models.contains_key(name) {
            let p = self.pmd.props.get(name).ok_or(ParseError {
                pos: 0,
                msg: format!("model {} not found", name),
            })?;
            let payload = p.payload().ok_or(ParseError {
                pos: 0,
                msg: format!("model {} is not an ObjectProperty", name),
            })?;
            let m = Model::parse(payload)?;
            self.models.insert(name.to_string(), m);
        }
        Ok(self.models.get_mut(name).unwrap())
    }

    pub fn to_bytes(&mut self) -> Vec<u8> {
        for (name, m) in &self.models {
            if let Some(p) = self.pmd.props.get_mut(name) {
                if let Value::Scalar { payload, .. } = &mut p.value {
                    *payload = m.to_bytes();
                }
            }
        }
        let pmd_bytes = self.pmd.to_bytes();
        if let Some(p) = self.top.get_mut("PersistentModelData") {
            if let Value::Array { body, .. } = &mut p.value {
                *body = ArrayBody::Raw(pmd_bytes);
            }
        }
        let mut w = Writer::new();
        write_list(&mut w, &self.top);
        w.write(&self.trailer);
        w.into_bytes()
    }
}
