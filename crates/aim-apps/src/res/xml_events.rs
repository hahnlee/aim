//! The element/text events of ResXMLTree, without flattening mixed content.
use super::*;

#[derive(Debug, PartialEq)]
pub enum XmlEvent {
    Start(String),
    End(String),
    Text(String),
}

pub struct XmlEvents<'a> {
    remaining: &'a [u8],
    strings: Option<Strings>,
    stack: Vec<(u32, u32)>,
    root: bool,
    finished: bool,
}

impl<'a> XmlEvents<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self> {
        let top = chunks(data)
            .next()
            .ok_or_else(|| bad("empty binary XML"))??;
        if top.kind != XML || top.header < 8 || top.data.len() != data.len() {
            return Err(bad("bad binary XML document"));
        }
        Ok(Self {
            remaining: &top.data[top.header..],
            strings: None,
            stack: Vec::new(),
            root: false,
            finished: false,
        })
    }

    fn string(&self, index: u32) -> Result<String> {
        self.strings
            .as_ref()
            .and_then(|pool| pool.get(index))
            .map(str::to_owned)
            .ok_or_else(|| bad("invalid binary XML string index"))
    }

    fn event(&mut self) -> Result<Option<XmlEvent>> {
        loop {
            if self.remaining.is_empty() {
                self.finished = true;
                return if self.stack.is_empty() && self.root {
                    Ok(None)
                } else {
                    Err(bad("incomplete binary XML document"))
                };
            }
            let c = chunks(self.remaining)
                .next()
                .ok_or_else(|| bad("truncated binary XML chunk"))??;
            if c.header < 8 {
                return Err(bad("bad binary XML chunk header"));
            }
            self.remaining = &self.remaining[c.data.len()..];
            match c.kind {
                STRING_POOL => {
                    if self.strings.is_some() || self.root {
                        return Err(bad("duplicate binary XML string pool"));
                    }
                    self.strings = Some(Strings::parse(c.data)?);
                }
                XML_START_ELEMENT | XML_END_ELEMENT => {
                    if c.header < 16 {
                        return Err(bad("truncated binary XML node header"));
                    }
                    let ns = u32_at(c.data, c.header)?;
                    let id = u32_at(c.data, c.header + 4)?;
                    let name = self.string(id)?;
                    if ns != u32::MAX {
                        self.string(ns)?;
                    }
                    if c.kind == XML_START_ELEMENT {
                        let start = u16_at(c.data, c.header + 8)? as usize;
                        let size = u16_at(c.data, c.header + 10)? as usize;
                        let count = u16_at(c.data, c.header + 12)? as usize;
                        u32_at(c.data, c.header + 16)?;
                        if start < 20 || size < 20 || c.header + start + size * count > c.data.len()
                        {
                            return Err(bad("truncated binary XML attributes"));
                        }
                        if self.stack.is_empty() && self.root {
                            return Err(bad("multiple binary XML roots"));
                        }
                        self.root = true;
                        self.stack.push((ns, id));
                        return Ok(Some(XmlEvent::Start(name)));
                    }
                    if self.stack.pop() != Some((ns, id)) {
                        return Err(bad("unbalanced binary XML"));
                    }
                    return Ok(Some(XmlEvent::End(name)));
                }
                0x0104 => {
                    if c.header < 16 || self.stack.is_empty() {
                        return Err(bad("binary XML text outside an element"));
                    }
                    // ResXMLTree_cdataExt: data is the raw string-pool index.
                    let text = self.string(u32_at(c.data, c.header)?)?;
                    u32_at(c.data, c.header + 8)?;
                    return Ok(Some(XmlEvent::Text(text)));
                }
                0x0100 | 0x0101 | XML_RESOURCE_MAP => {}
                _ => return Err(bad("unknown binary XML chunk")),
            }
        }
    }
}

impl Iterator for XmlEvents<'_> {
    type Item = Result<XmlEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        match self.event() {
            Ok(event) => event.map(Ok),
            Err(error) => {
                self.finished = true;
                Some(Err(error))
            }
        }
    }
}
