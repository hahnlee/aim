//! A reader of Android's binary XML (ABX, `BinaryXmlSerializer`), the
//! format system_server writes its settings in (`packages.xml`,
//! `package-restrictions.xml`): each element with its attributes, in
//! document order.

/// An element: its depth (the root's is 0), name and attributes, values as
/// `XmlPullParser` reads them back as strings.
#[derive(Debug, PartialEq)]
pub struct Element {
    pub depth: usize,
    pub name: String,
    pub attrs: Vec<(String, String)>,
}

impl Element {
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

const MAGIC: &[u8] = b"ABX\0";
const START_DOCUMENT: u8 = 0;
const END_DOCUMENT: u8 = 1;
const START_TAG: u8 = 2;
const END_TAG: u8 = 3;
const ATTRIBUTE: u8 = 15;
const TYPE_NULL: u8 = 1;
const TYPE_STRING: u8 = 2;
const TYPE_STRING_INTERNED: u8 = 3;
const TYPE_BYTES_HEX: u8 = 4;
const TYPE_BYTES_BASE64: u8 = 5;
const TYPE_INT: u8 = 6;
const TYPE_INT_HEX: u8 = 7;
const TYPE_LONG: u8 = 8;
const TYPE_LONG_HEX: u8 = 9;
const TYPE_FLOAT: u8 = 10;
const TYPE_DOUBLE: u8 = 11;
const TYPE_BOOLEAN_TRUE: u8 = 12;
const TYPE_BOOLEAN_FALSE: u8 = 13;

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    interned: Vec<String>,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self.at + n;
        let out = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| format!("ABX: truncated at {}", self.at))?;
        self.at = end;
        Ok(out)
    }

    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }

    fn utf(&mut self) -> Result<String, String> {
        let len = self.u16()? as usize;
        // Java's modified UTF-8 differs from UTF-8 only in NUL and
        // supplementary characters, which settings do not hold.
        Ok(String::from_utf8_lossy(self.take(len)?).into_owned())
    }

    fn interned(&mut self) -> Result<String, String> {
        match self.u16()? {
            0xffff => {
                let s = self.utf()?;
                self.interned.push(s.clone());
                Ok(s)
            }
            i => self
                .interned
                .get(i as usize)
                .cloned()
                .ok_or_else(|| format!("ABX: no interned string {i}")),
        }
    }

    fn value(&mut self, kind: u8) -> Result<String, String> {
        let hex = |b: &[u8]| b.iter().map(|b| format!("{b:02x}")).collect::<String>();
        Ok(match kind {
            TYPE_NULL => String::new(),
            TYPE_STRING => self.utf()?,
            TYPE_STRING_INTERNED => self.interned()?,
            TYPE_BYTES_HEX | TYPE_BYTES_BASE64 => {
                let len = self.u16()? as usize;
                hex(self.take(len)?)
            }
            TYPE_INT => i32::from_be_bytes(self.take(4)?.try_into().unwrap()).to_string(),
            TYPE_INT_HEX => format!(
                "{:x}",
                i32::from_be_bytes(self.take(4)?.try_into().unwrap())
            ),
            TYPE_LONG => i64::from_be_bytes(self.take(8)?.try_into().unwrap()).to_string(),
            TYPE_LONG_HEX => format!(
                "{:x}",
                i64::from_be_bytes(self.take(8)?.try_into().unwrap())
            ),
            TYPE_FLOAT => f32::from_be_bytes(self.take(4)?.try_into().unwrap()).to_string(),
            TYPE_DOUBLE => f64::from_be_bytes(self.take(8)?.try_into().unwrap()).to_string(),
            TYPE_BOOLEAN_TRUE => "true".into(),
            TYPE_BOOLEAN_FALSE => "false".into(),
            other => return Err(format!("ABX: value type {other}")),
        })
    }
}

/// The elements of the ABX document `bytes`.
pub fn elements(bytes: &[u8]) -> Result<Vec<Element>, String> {
    if !bytes.starts_with(MAGIC) {
        return Err("not ABX".into());
    }
    let mut r = Reader {
        bytes,
        at: MAGIC.len(),
        interned: Vec::new(),
    };
    let (mut out, mut open) = (Vec::<Element>::new(), Vec::<usize>::new());
    while r.at < bytes.len() {
        let token = r.take(1)?[0];
        let (event, kind) = (token & 0x0f, token >> 4);
        match event {
            START_TAG => {
                let name = r.interned()?;
                open.push(out.len());
                out.push(Element {
                    depth: open.len() - 1,
                    name,
                    attrs: Vec::new(),
                });
            }
            ATTRIBUTE => {
                let name = r.interned()?;
                let value = r.value(kind)?;
                let element = open.last().ok_or("ABX: an attribute outside an element")?;
                out[*element].attrs.push((name, value));
            }
            END_TAG => {
                r.interned()?;
                open.pop().ok_or("ABX: an end tag outside an element")?;
            }
            START_DOCUMENT | END_DOCUMENT => {}
            // Text, comments and the like: a string, or none.
            _ => {
                r.value(kind)?;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_elements_and_typed_attributes() {
        let mut b = MAGIC.to_vec();
        let utf = |b: &mut Vec<u8>, s: &str| {
            b.extend((s.len() as u16).to_be_bytes());
            b.extend(s.as_bytes());
        };
        let new = |b: &mut Vec<u8>, s: &str| {
            b.extend([0xff, 0xff]);
            utf(b, s);
        };
        b.push(START_DOCUMENT | TYPE_NULL << 4);
        b.push(START_TAG | TYPE_STRING_INTERNED << 4);
        new(&mut b, "packages");
        b.push(START_TAG | TYPE_STRING_INTERNED << 4);
        new(&mut b, "package");
        b.push(ATTRIBUTE | TYPE_STRING << 4);
        new(&mut b, "name");
        utf(&mut b, "android");
        b.push(ATTRIBUTE | TYPE_INT << 4);
        new(&mut b, "userId");
        b.extend(1000i32.to_be_bytes());
        b.push(ATTRIBUTE | TYPE_LONG_HEX << 4);
        new(&mut b, "ft");
        b.extend(0x1234i64.to_be_bytes());
        b.push(ATTRIBUTE | TYPE_BOOLEAN_TRUE << 4);
        new(&mut b, "isOrphaned");
        b.push(END_TAG | TYPE_STRING_INTERNED << 4);
        b.extend(1u16.to_be_bytes());
        b.push(START_TAG | TYPE_STRING_INTERNED << 4);
        b.extend(1u16.to_be_bytes());
        b.push(ATTRIBUTE | TYPE_STRING_INTERNED << 4);
        b.extend(2u16.to_be_bytes());
        b.extend(0u16.to_be_bytes());
        b.push(END_TAG | TYPE_STRING_INTERNED << 4);
        b.extend(1u16.to_be_bytes());
        b.push(END_TAG | TYPE_STRING_INTERNED << 4);
        b.extend(0u16.to_be_bytes());
        b.push(END_DOCUMENT | TYPE_NULL << 4);

        let got = elements(&b).unwrap();
        assert_eq!(got.len(), 3);
        assert_eq!((got[0].depth, got[0].name.as_str()), (0, "packages"));
        assert_eq!(got[1].depth, 1);
        assert_eq!(got[1].attr("name"), Some("android"));
        assert_eq!(got[1].attr("userId"), Some("1000"));
        assert_eq!(got[1].attr("ft"), Some("1234"));
        assert_eq!(got[1].attr("isOrphaned"), Some("true"));
        assert_eq!(got[2].attr("name"), Some("packages"));
        assert!(elements(b"<?xml").is_err());
    }
}
