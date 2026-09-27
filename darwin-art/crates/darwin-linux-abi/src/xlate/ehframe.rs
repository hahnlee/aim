//! Function ranges from `.eh_frame`: every FDE covers
//! `[pc_begin, pc_begin + pc_range)` of code. Stripped Android libraries keep
//! their unwind tables, so this is the code evidence available for internal
//! functions that have no symbol. Unknown or malformed records end the walk.

const DW_EH_PE_OMIT: u8 = 0xff;

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn fixed<const N: usize>(&mut self) -> Option<[u8; N]> {
        let v = self.b.get(self.pos..self.pos + N)?.try_into().ok()?;
        self.pos += N;
        Some(v)
    }
    fn uleb(&mut self) -> Option<u64> {
        let (mut v, mut shift) = (0u64, 0);
        loop {
            let byte = self.u8()?;
            if shift < 64 {
                v |= ((byte & 0x7f) as u64) << shift;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                return Some(v);
            }
        }
    }
    fn sleb(&mut self) -> Option<i64> {
        let (mut v, mut shift) = (0i64, 0);
        loop {
            let byte = self.u8()?;
            if shift < 64 {
                v |= ((byte & 0x7f) as i64) << shift;
            }
            shift += 7;
            if byte & 0x80 == 0 {
                if shift < 64 && byte & 0x40 != 0 {
                    v |= -1i64 << shift;
                }
                return Some(v);
            }
        }
    }
    fn cstr(&mut self) -> Option<&[u8]> {
        let rest = self.b.get(self.pos..)?;
        let n = rest.iter().position(|&c| c == 0)?;
        self.pos += n + 1;
        Some(&rest[..n])
    }
    /// A pointer in encoding `enc`; `base` is the address of `b[0]`.
    fn pointer(&mut self, enc: u8, base: u64) -> Option<u64> {
        let at = base.wrapping_add(self.pos as u64);
        let raw = match enc & 0x0f {
            0x00 => u64::from_le_bytes(self.fixed::<8>()?),
            0x01 => self.uleb()?,
            0x02 => u16::from_le_bytes(self.fixed::<2>()?) as u64,
            0x03 => u32::from_le_bytes(self.fixed::<4>()?) as u64,
            0x04 => u64::from_le_bytes(self.fixed::<8>()?),
            0x09 => self.sleb()? as u64,
            0x0a => i16::from_le_bytes(self.fixed::<2>()?) as i64 as u64,
            0x0b => i32::from_le_bytes(self.fixed::<4>()?) as i64 as u64,
            0x0c => u64::from_le_bytes(self.fixed::<8>()?),
            _ => return None,
        };
        match enc & 0x70 {
            0x00 => Some(raw),
            0x10 => Some(at.wrapping_add(raw)),
            _ => None,
        }
    }
}

struct Cie {
    fde_enc: u8,
    has_aug_data: bool,
}

fn parse_cie(b: &[u8], start: usize, end: usize, base: u64) -> Option<Cie> {
    let mut r = Reader {
        b: b.get(..end)?,
        pos: start,
    };
    let version = r.u8()?;
    let aug = r.cstr()?.to_vec();
    if aug.windows(2).any(|w| w == b"eh") {
        r.fixed::<8>()?;
    }
    r.uleb()?; // code alignment
    r.sleb()?; // data alignment
    if version == 1 {
        r.u8()?;
    } else {
        r.uleb()?;
    }
    let mut cie = Cie {
        fde_enc: 0,
        has_aug_data: aug.first() == Some(&b'z'),
    };
    if cie.has_aug_data {
        r.uleb()?;
        for &c in &aug[1..] {
            match c {
                b'R' => cie.fde_enc = r.u8()?,
                b'L' => {
                    r.u8()?;
                }
                b'P' => {
                    let enc = r.u8()?;
                    r.pointer(enc & 0x7f, base)?;
                }
                b'S' | b'B' | b'G' => {}
                _ => return None,
            }
        }
    }
    Some(cie)
}

/// FDE ranges of an `.eh_frame` section whose first byte is at `base`.
pub fn fde_ranges(b: &[u8], base: u64) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let mut cies: std::collections::HashMap<usize, Option<Cie>> = Default::default();
    let mut pos = 0usize;
    while pos + 4 <= b.len() {
        let len32 = u32::from_le_bytes(b[pos..pos + 4].try_into().unwrap());
        if len32 == 0 {
            break;
        }
        let (len, hdr) = if len32 == 0xffff_ffff {
            let Some(l) = b.get(pos + 4..pos + 12) else {
                break;
            };
            (u64::from_le_bytes(l.try_into().unwrap()) as usize, 12)
        } else {
            (len32 as usize, 4)
        };
        let body = pos + hdr;
        let Some(end) = body.checked_add(len).filter(|&e| e <= b.len()) else {
            break;
        };
        let Some(id) = b.get(body..body + 4) else {
            break;
        };
        let id = u32::from_le_bytes(id.try_into().unwrap()) as usize;
        if id != 0 {
            // FDE: the CIE pointer is relative to this field.
            if let Some(cie_pos) = body.checked_sub(id) {
                let cie = cies.entry(cie_pos).or_insert_with(|| {
                    let l = u32::from_le_bytes(b.get(cie_pos..cie_pos + 4)?.try_into().ok()?);
                    let cie_end = (cie_pos + 4).checked_add(l as usize)?;
                    parse_cie(b, cie_pos + 8, cie_end, base)
                });
                if let Some(cie) = cie
                    && cie.fde_enc != DW_EH_PE_OMIT
                {
                    let mut r = Reader {
                        b: &b[..end],
                        pos: body + 4,
                    };
                    if let (Some(begin), Some(range)) = (
                        r.pointer(cie.fde_enc, base),
                        r.pointer(cie.fde_enc & 0x0f, base),
                    ) && range > 0
                    {
                        out.push((begin, begin.saturating_add(range)));
                    }
                }
            }
        }
        pos = end;
    }
    out
}
