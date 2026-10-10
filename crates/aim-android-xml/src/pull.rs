//! Incremental `XmlPullParser.next()` for persistence owners. Earlier events
//! remain observable when a later read fails; callers choose where to stop.
//! Binary event/EOF behavior follows android-16.0.0_r1 BinaryXmlPullParser
//! (The Android Open Source Project, Apache License 2.0).
use crate::{CDSECT, COMMENT, ENTITY_REF, Element, Node, PROCESSING_INSTRUCTION, TEXT};

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Start(Element),
    End(String),
    Text(String),
    EndDocument,
}

pub(crate) enum Token {
    Start(Element),
    End(String),
    Content(u8, Option<String>),
    EndDocument,
}

enum Input<'a> {
    Binary(crate::abx::Pull<'a>),
    Text(crate::text::Pull<'a>),
}

pub struct Reader<'a> {
    input: Input<'a>,
    depth: i32,
    end_tag: bool,
    ended: bool,
    document: Option<Document>,
}

#[derive(Default)]
struct Document {
    stack: Vec<Element>,
    root: Option<Element>,
    complete: bool,
    failed: bool,
}

impl Document {
    fn close(&mut self) {
        if let Some(element) = self.stack.pop() {
            if let Some(parent) = self.stack.last_mut() {
                parent.content.push(Node::Element(element));
            } else {
                self.root = Some(element);
                self.complete = true;
            }
        }
    }

    fn token(&mut self, token: &Token) {
        if self.complete {
            return;
        }
        match token {
            Token::Start(element) => self.stack.push(element.clone()),
            Token::End(_) => self.close(),
            Token::Content(kind, text) => {
                if let Some(element) = self.stack.last_mut() {
                    element.content.push(Node::Token(*kind, text.clone()));
                }
            }
            Token::EndDocument => {
                while !self.stack.is_empty() {
                    self.close();
                }
                self.complete = true;
            }
        }
    }
}

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, String> {
        Ok(Self {
            input: if bytes.starts_with(crate::abx::MAGIC) {
                Input::Binary(crate::abx::Pull::new(bytes)?)
            } else {
                Input::Text(crate::text::Pull::new(bytes)?)
            },
            depth: 0,
            end_tag: false,
            ended: false,
            document: None,
        })
    }

    /// Capture the tokens consumed by this reader, including helper-owned and
    /// skipped subtrees. No second parse or read past the owner's stop boundary.
    pub fn with_document(bytes: &'a [u8]) -> Result<Self, String> {
        let mut reader = Self::new(bytes)?;
        reader.document = Some(Document::default());
        Ok(reader)
    }

    pub fn take_document(&mut self) -> Result<Option<Element>, String> {
        let mut document = self
            .document
            .take()
            .ok_or("document capture is not enabled")?;
        if document.failed {
            return Err("document capture failed during XML reading".into());
        }
        if !document.complete {
            return Err("document capture has not reached root end or EOF".into());
        }
        if let Some(root) = &mut document.root {
            crate::normalize_next(root, matches!(self.input, Input::Binary(_)))?;
        }
        Ok(document.root)
    }

    /// SettingsXml.moveToNext explicitly catches an XML exception and keeps
    /// reading its cursor. Call only after recording that section's diagnostic;
    /// unaccepted errors continue to prevent document export.
    pub fn resume_document_after_section_error(&mut self) {
        if let Some(document) = &mut self.document {
            document.failed = false;
        }
    }

    /// END_TAG retains its element's depth until the next event, as Android does.
    pub fn depth(&self) -> i32 {
        self.depth
    }

    fn token(&mut self) -> Result<Token, String> {
        let token = match &mut self.input {
            Input::Binary(reader) => reader.token(),
            Input::Text(reader) => reader.token(),
        }?;
        if let Some(document) = &mut self.document {
            document.token(&token);
        }
        Ok(token)
    }

    fn peek(&self) -> Result<u8, String> {
        match &self.input {
            Input::Binary(reader) => reader.peek(),
            Input::Text(reader) => reader.peek(),
        }
    }

    pub fn next(&mut self) -> Result<Event, String> {
        let result = self.next_event();
        if result.is_err()
            && let Some(document) = &mut self.document
        {
            document.failed = true;
        }
        result
    }

    fn next_event(&mut self) -> Result<Event, String> {
        if self.ended {
            return Ok(Event::EndDocument);
        }
        if self.end_tag {
            self.depth -= 1;
            self.end_tag = false;
        }
        loop {
            match self.token()? {
                Token::Start(element) => {
                    self.depth += 1;
                    return Ok(Event::Start(element));
                }
                Token::End(name) => {
                    self.end_tag = true;
                    return Ok(Event::End(name));
                }
                Token::EndDocument => {
                    self.ended = true;
                    return Ok(Event::EndDocument);
                }
                Token::Content(kind, text)
                    if kind == TEXT || (kind == CDSECT && matches!(self.input, Input::Text(_))) =>
                {
                    let mut combined = text.unwrap_or_default();
                    while matches!(
                        self.peek()?,
                        TEXT | CDSECT | ENTITY_REF | COMMENT | PROCESSING_INSTRUCTION
                    ) {
                        if let Token::Content(kind, text) = self.token()? {
                            match kind {
                                TEXT | CDSECT => combined.push_str(&text.unwrap_or_default()),
                                _ => {}
                            }
                        }
                    }
                    if !combined.is_empty() {
                        return Ok(Event::Text(combined));
                    }
                }
                _ => {}
            }
        }
    }
}

pub(crate) fn entity(name: &str) -> Result<String, String> {
    Ok(match name {
        "lt" => "<".into(),
        "gt" => ">".into(),
        "amp" => "&".into(),
        "apos" => "'".into(),
        "quot" => "\"".into(),
        _ => {
            // BinaryXmlPullParser parses decimal references and casts to char.
            let value: i32 = name
                .strip_prefix('#')
                .ok_or_else(|| format!("ABX: unknown entity {name}"))?
                .parse()
                .map_err(|_| format!("ABX: unknown entity {name}"))?;
            String::from_utf16(&[value as u16])
                .map_err(|_| format!("ABX: unpaired entity surrogate {name}"))?
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_preserves_text_comments_cdata_entities_and_binary_attribute_types() {
        let bytes =
            b"<r kind='value'>a<!--comment--><![CDATA[b]]><?probe value?>&amp;<child/>tail</r>";
        let mut root = crate::read(bytes).unwrap();
        root.attrs.push(("number".into(), crate::Value::IntHex(42)));
        for bytes in [bytes.to_vec(), crate::abx::write(&root).unwrap()] {
            let mut reader = Reader::with_document(&bytes).unwrap();
            loop {
                match reader.next().unwrap() {
                    Event::End(_) if reader.depth() == 1 => break,
                    Event::EndDocument => break,
                    _ => {}
                }
            }
            assert_eq!(
                reader.take_document().unwrap().unwrap(),
                crate::read_next(&bytes).unwrap()
            );
        }
    }

    #[test]
    fn captured_document_obeys_root_stop_eof_and_failed_read_boundaries() {
        for bytes in [b"<r><child/></r>broken".as_slice(), b"<r><child/>"] {
            let mut reader = Reader::with_document(bytes).unwrap();
            loop {
                match reader.next().unwrap() {
                    Event::End(_) if reader.depth() == 1 => break,
                    Event::EndDocument => break,
                    _ => {}
                }
            }
            let root = reader.take_document().unwrap().unwrap();
            assert_eq!(root.name, "r");
            assert_eq!(root.children().next().unwrap().name, "child");
            assert!(reader.take_document().is_err());
        }
        let mut reader = Reader::with_document(b"<r><").unwrap();
        reader.next().unwrap();
        assert!(reader.next().is_err());
        assert!(reader.take_document().is_err());
        let mut reader = Reader::with_document(b" ").unwrap();
        assert_eq!(reader.next().unwrap(), Event::EndDocument);
        assert!(reader.take_document().unwrap().is_none());
    }

    #[test]
    fn completed_events_survive_a_later_failure_and_root_end_is_a_stop_boundary() {
        let bytes = b"<packages><version sdkVersion='36'/><package name='p'><";
        let mut reader = Reader::new(bytes).unwrap();
        for (name, depth) in [("packages", 1), ("version", 2)] {
            let Event::Start(element) = reader.next().unwrap() else {
                panic!()
            };
            assert_eq!(element.name, name);
            assert_eq!(reader.depth(), depth);
        }
        assert_eq!(reader.next().unwrap(), Event::End("version".into()));
        assert_eq!(reader.depth(), 2);
        let Event::Start(element) = reader.next().unwrap() else {
            panic!()
        };
        assert_eq!(element.string("name").as_deref(), Some("p"));
        assert!(reader.next().is_err());

        let mut reader = Reader::new(b"<packages>").unwrap();
        assert!(matches!(reader.next().unwrap(), Event::Start(_)));
        assert_eq!(reader.next().unwrap(), Event::EndDocument);
        let mut reader = Reader::new(b"broken").unwrap();
        assert!(reader.next().is_err());

        let mut reader = Reader::new(b"<packages/>broken").unwrap();
        assert!(matches!(reader.next().unwrap(), Event::Start(_)));
        assert_eq!(reader.next().unwrap(), Event::End("packages".into()));
        assert!(reader.next().is_err());
    }

    #[test]
    fn binary_attributes_are_available_before_the_body_and_eof_matches_next_token() {
        let document = crate::read(b"<packages><package name='p'/></packages>").unwrap();
        let bytes = crate::abx::write(&document).unwrap();
        let mut reader = Reader::new(&bytes[..bytes.len() - 1]).unwrap();
        assert!(matches!(reader.next().unwrap(), Event::Start(_)));
        let Event::Start(element) = reader.next().unwrap() else {
            panic!()
        };
        assert_eq!(element.string("name").as_deref(), Some("p"));
        assert_eq!(reader.next().unwrap(), Event::End("package".into()));
        assert_eq!(reader.next().unwrap(), Event::End("packages".into()));
        assert_eq!(reader.next().unwrap(), Event::EndDocument);
        assert!(Reader::new(b"ABX\0").is_err());
    }

    #[test]
    fn next_coalesces_text_and_cdata_over_comments() {
        let mut reader = Reader::new(b"<p>a<!--x--><![CDATA[b]]>c</p>").unwrap();
        assert!(matches!(reader.next().unwrap(), Event::Start(_)));
        assert_eq!(reader.next().unwrap(), Event::Text("abc".into()));
        assert_eq!(reader.next().unwrap(), Event::End("p".into()));
    }
}
