//! Little-endian byte reader/writer with UE4 FString support.

use std::fmt;

#[derive(Debug)]
pub struct ParseError {
    pub pos: usize,
    pub msg: String,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at offset {:#x}", self.msg, self.pos)
    }
}

impl std::error::Error for ParseError {}

pub type Result<T> = std::result::Result<T, ParseError>;

pub struct Reader<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T> {
        Err(ParseError { pos: self.pos, msg: msg.into() })
    }

    pub fn read(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return self.err(format!("wanted {} bytes, {} left", n, self.remaining()));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.read(1)?[0])
    }

    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.read(4)?.try_into().unwrap()))
    }

    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read(4)?.try_into().unwrap()))
    }

    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.read(8)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.read(4)?.try_into().unwrap()))
    }

    /// Length-checked read of `size` declared by a tag, guarding against
    /// negative or oversized values from truncated/corrupt files.
    pub fn sized(&mut self, size: i64) -> Result<&'a [u8]> {
        if size < 0 {
            return self.err(format!("negative size {}", size));
        }
        let n = size as usize;
        if n > self.remaining() {
            return self.err(format!("declared size {} exceeds {} remaining", n, self.remaining()));
        }
        self.read(n)
    }

    /// UE4 FString: i32 length; positive = ASCII with NUL, negative = UTF-16LE with NUL.
    pub fn fstring(&mut self) -> Result<String> {
        let n = self.i32()?;
        if n == 0 {
            return Ok(String::new());
        }
        if n > 0 {
            let raw = self.read(n as usize)?;
            if raw.last() != Some(&0) {
                return self.err("FString missing NUL terminator");
            }
            Ok(raw[..raw.len() - 1].iter().map(|&b| b as char).collect())
        } else {
            let chars = (-(n as i64)) as usize;
            let raw = self.read(chars * 2)?;
            let units: Vec<u16> = raw
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            let units = &units[..units.len().saturating_sub(1)];
            Ok(String::from_utf16_lossy(units))
        }
    }
}

#[derive(Default)]
pub struct Writer {
    pub buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Writer::default()
    }

    pub fn write(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }

    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }

    pub fn i32(&mut self, v: i32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }

    pub fn fstring(&mut self, s: &str) {
        if s.is_empty() {
            self.i32(0);
            return;
        }
        if s.is_ascii() {
            self.i32((s.len() + 1) as i32);
            self.write(s.as_bytes());
            self.u8(0);
        } else {
            let units: Vec<u16> = s.encode_utf16().collect();
            self.i32(-((units.len() + 1) as i32));
            for u in units {
                self.write(&u.to_le_bytes());
            }
            self.write(&[0, 0]);
        }
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}
