//! Minimal, bounds-checked TIFF/IFD reader. Used for EXIF tags and for locating uncompressed
//! previews and raw-image sizes in TIFF-based RAW containers. Never panics on malformed input.

/// A TIFF structure inside a byte buffer. Offsets in IFD entries are relative to `base`.
#[derive(Debug, Clone, Copy)]
pub struct Tiff<'a> {
    buf: &'a [u8],
    base: usize,
    le: bool,
    first_ifd: usize,
}

/// One IFD entry.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    /// Absolute offset in the buffer where the value bytes start.
    data: usize,
}

/// A parsed IFD.
#[derive(Debug, Clone, Default)]
pub struct Ifd {
    pub entries: Vec<Entry>,
    pub next: usize,
}

const MAX_ENTRIES: usize = 4096;

fn type_size(typ: u16) -> usize {
    match typ {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

impl<'a> Tiff<'a> {
    /// Parse a TIFF header at `base`. Accepts standard TIFF (`II*\0`, `MM\0*`), Olympus ORF
    /// (`IIRO`, `IIRS`, `MMOR`) and Panasonic RW2 (`IIU\0`) magics.
    pub fn parse(buf: &'a [u8], base: usize) -> Option<Self> {
        let h = buf.get(base..base + 8)?;
        let le = match &h[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let magic = if le {
            u16::from_le_bytes([h[2], h[3]])
        } else {
            u16::from_be_bytes([h[2], h[3]])
        };
        if !matches!(magic, 42 | 0x4f52 | 0x5352 | 0x0055) {
            return None;
        }
        let t = Self {
            buf,
            base,
            le,
            first_ifd: 0,
        };
        let first = t.u32_at(base + 4)? as usize;
        Some(Self {
            first_ifd: first,
            ..t
        })
    }

    pub fn buf(&self) -> &'a [u8] {
        self.buf
    }

    pub fn base(&self) -> usize {
        self.base
    }

    /// Offset (relative to base) of the first IFD.
    pub fn first_ifd(&self) -> usize {
        self.first_ifd
    }

    fn u16_at(&self, at: usize) -> Option<u16> {
        let b = self.buf.get(at..at + 2)?;
        Some(if self.le {
            u16::from_le_bytes([b[0], b[1]])
        } else {
            u16::from_be_bytes([b[0], b[1]])
        })
    }

    fn u32_at(&self, at: usize) -> Option<u32> {
        let b = self.buf.get(at..at + 4)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }

    /// Read the IFD at `off` (relative to base).
    pub fn ifd(&self, off: usize) -> Option<Ifd> {
        if off == 0 {
            return None;
        }
        let start = self.base.checked_add(off)?;
        let n = self.u16_at(start)? as usize;
        if n == 0 || n > MAX_ENTRIES {
            return None;
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let e = start + 2 + i * 12;
            let tag = self.u16_at(e)?;
            let typ = self.u16_at(e + 2)?;
            let count = self.u32_at(e + 4)?;
            let size = type_size(typ).checked_mul(count as usize)?;
            let data = if size <= 4 {
                e + 8
            } else {
                self.base.checked_add(self.u32_at(e + 8)? as usize)?
            };
            entries.push(Entry {
                tag,
                typ,
                count,
                data,
            });
        }
        let next = self.u32_at(start + 2 + n * 12).unwrap_or(0) as usize;
        Some(Ifd { entries, next })
    }

    /// Integer values of an entry (BYTE, SHORT, LONG, and their signed forms), at most `max`.
    pub fn uints(&self, e: &Entry, max: usize) -> Vec<u64> {
        let n = (e.count as usize).min(max);
        let mut out = Vec::with_capacity(n);
        for i in 0..n {
            let v = match e.typ {
                1 | 7 => self.buf.get(e.data + i).map(|&b| u64::from(b)),
                6 => self.buf.get(e.data + i).map(|&b| b as i8 as i64 as u64),
                3 => self.u16_at(e.data + i * 2).map(u64::from),
                8 => self.u16_at(e.data + i * 2).map(|v| v as i16 as i64 as u64),
                4 | 13 => self.u32_at(e.data + i * 4).map(u64::from),
                9 => self.u32_at(e.data + i * 4).map(|v| v as i32 as i64 as u64),
                _ => None,
            };
            match v {
                Some(v) => out.push(v),
                None => break,
            }
        }
        out
    }

    /// First integer value of an entry.
    pub fn uint(&self, e: &Entry) -> Option<u64> {
        self.uints(e, 1).first().copied()
    }

    /// First RATIONAL / SRATIONAL value as f64 (None for a zero denominator).
    pub fn rational(&self, e: &Entry) -> Option<f64> {
        match e.typ {
            5 => {
                let n = self.u32_at(e.data)?;
                let d = self.u32_at(e.data + 4)?;
                (d != 0).then(|| f64::from(n) / f64::from(d))
            }
            10 => {
                let n = self.u32_at(e.data)? as i32;
                let d = self.u32_at(e.data + 4)? as i32;
                (d != 0).then(|| f64::from(n) / f64::from(d))
            }
            _ => self.uint(e).map(|v| v as f64),
        }
    }

    /// ASCII value, trimmed of NULs and surrounding whitespace. None if empty.
    pub fn ascii(&self, e: &Entry) -> Option<String> {
        if !matches!(e.typ, 2 | 7 | 1) {
            return None;
        }
        let bytes = self
            .buf
            .get(e.data..e.data.checked_add(e.count as usize)?)?;
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        let s = String::from_utf8_lossy(&bytes[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }
}

impl Ifd {
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

/// Walk IFD0's chain and every SubIFD (tag 0x14A), depth-limited, calling `f` for each IFD.
pub fn walk_image_ifds(t: &Tiff<'_>, mut f: impl FnMut(&Ifd)) {
    let mut seen = Vec::new();
    let mut stack = vec![(t.first_ifd(), 0usize)];
    while let Some((off, depth)) = stack.pop() {
        if off == 0 || depth > 4 || seen.contains(&off) || seen.len() > 64 {
            continue;
        }
        seen.push(off);
        let Some(ifd) = t.ifd(off) else { continue };
        f(&ifd);
        if let Some(e) = ifd.get(0x14A) {
            for sub in t.uints(e, 16) {
                stack.push((sub as usize, depth + 1));
            }
        }
        stack.push((ifd.next, depth));
    }
}

#[cfg(test)]
pub(crate) mod build {
    //! Tiny little-endian TIFF writer for unit tests.

    /// (tag, type, count, value bytes)
    pub type Field = (u16, u16, u32, Vec<u8>);

    /// Build a TIFF with one IFD (plus optional extra IFDs referenced by pointer tags that the
    /// caller patches in). Returns bytes; IFD0 starts at offset 8.
    pub fn tiff_le(fields: &[Field]) -> Vec<u8> {
        let mut fields = fields.to_vec();
        fields.sort_by_key(|f| f.0);
        let n = fields.len();
        let ifd_len = 2 + n * 12 + 4;
        let mut out = b"II*\0".to_vec();
        out.extend_from_slice(&8u32.to_le_bytes());
        let mut data = Vec::new();
        let data_base = 8 + ifd_len;
        let mut ifd = (n as u16).to_le_bytes().to_vec();
        for (tag, typ, count, val) in &fields {
            ifd.extend_from_slice(&tag.to_le_bytes());
            ifd.extend_from_slice(&typ.to_le_bytes());
            ifd.extend_from_slice(&count.to_le_bytes());
            if val.len() <= 4 {
                let mut v = val.clone();
                v.resize(4, 0);
                ifd.extend_from_slice(&v);
            } else {
                let off = (data_base + data.len()) as u32;
                ifd.extend_from_slice(&off.to_le_bytes());
                data.extend_from_slice(val);
                if data.len() % 2 == 1 {
                    data.push(0);
                }
            }
        }
        ifd.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&ifd);
        out.extend_from_slice(&data);
        out
    }

    pub fn short(tag: u16, v: u16) -> Field {
        (tag, 3, 1, v.to_le_bytes().to_vec())
    }

    pub fn ascii(tag: u16, s: &str) -> Field {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        (tag, 2, b.len() as u32, b)
    }

    pub fn rational(tag: u16, n: u32, d: u32) -> Field {
        let mut b = n.to_le_bytes().to_vec();
        b.extend_from_slice(&d.to_le_bytes());
        (tag, 5, 1, b)
    }

    pub fn srational(tag: u16, n: i32, d: i32) -> Field {
        let mut b = n.to_le_bytes().to_vec();
        b.extend_from_slice(&d.to_le_bytes());
        (tag, 10, 1, b)
    }
}

#[cfg(test)]
mod tests {
    use super::build::*;
    use super::*;

    #[test]
    fn reads_values_and_rejects_garbage() {
        let b = tiff_le(&[
            short(0x112, 6),
            ascii(0x10F, "SONY"),
            rational(0x829A, 1, 200),
            srational(0x9204, -2, 3),
        ]);
        let t = Tiff::parse(&b, 0).unwrap();
        let ifd = t.ifd(t.first_ifd()).unwrap();
        assert_eq!(t.uint(ifd.get(0x112).unwrap()), Some(6));
        assert_eq!(t.ascii(ifd.get(0x10F).unwrap()).as_deref(), Some("SONY"));
        assert_eq!(t.rational(ifd.get(0x829A).unwrap()), Some(0.005));
        let ev = t.rational(ifd.get(0x9204).unwrap()).unwrap();
        assert!((ev + 2.0 / 3.0).abs() < 1e-9);
        assert!(Tiff::parse(b"garbage!", 0).is_none());
        // Truncated buffers never panic.
        for n in 0..b.len() {
            if let Some(t) = Tiff::parse(&b[..n], 0) {
                let _ = t.ifd(t.first_ifd());
            }
        }
    }
}
