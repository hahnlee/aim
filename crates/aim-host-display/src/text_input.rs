//! Text input from the Mac's input methods into an app's editor, between
//! the app host's native InputMethodManager (`aim_services::app_host`,
//! docs/input.md "IME"), the display server and the window hosts: the app
//! host says which task has an editor; the window showing it takes its
//! keys through the Mac's input method (`NSTextInputClient`), whose
//! marked and committed text and editing commands come back as the app's
//! `InputConnection` calls.
//!
//! The app host opens a connection with [`crate::wire::OP_TEXT_INPUT`];
//! the server answers with its mode (one `u32`), and from then on both
//! ends write [`Message`] frames: a little-endian `u32` length, then the
//! message. Between the server and a window host a frame follows a
//! [`crate::wire::host::TEXT_INPUT`] record.

use std::io::{self, Read, Write};

/// The largest frame either end accepts.
const MAX_FRAME: u32 = 1 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// App host to host: task `task` (-1: the display, in device mode) has
    /// an editor of `input_type` and `ime_options` (`EditorInfo`), or
    /// (`active` false) none.
    Editor { task: i32, active: bool, input_type: i32, ime_options: i32 },
    /// Host to app host: the input method's marked text, the cursor at
    /// `cursor` within it (`setComposingText`).
    Compose { text: String, cursor: i32 },
    /// Host to app host: text the input method committed (`commitText`).
    Commit { text: String },
    /// Host to app host: delete `before` characters before the cursor and
    /// `after` after it (`deleteSurroundingText`).
    Delete { before: i32, after: i32 },
    /// Host to app host: the editor's action (`performEditorAction`).
    Action { code: i32 },
    /// Host to app host: an Android key code, pressed and released
    /// (`sendKeyEvent`).
    Key { code: i32 },
}

const EDITOR: u8 = 1;
const COMPOSE: u8 = 2;
const COMMIT: u8 = 3;
const DELETE: u8 = 4;
const ACTION: u8 = 5;
const KEY: u8 = 6;

fn bad() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "malformed text input frame")
}

struct In<'a>(&'a [u8]);

impl In<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(bad());
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn str(&mut self) -> io::Result<String> {
        let n = self.i32()?;
        let bytes = self.take(usize::try_from(n).map_err(|_| bad())?)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| bad())
    }
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Vec::new();
        let i32 = |o: &mut Vec<u8>, v: i32| o.extend_from_slice(&v.to_le_bytes());
        let str = |o: &mut Vec<u8>, s: &str| {
            o.extend_from_slice(&(s.len() as i32).to_le_bytes());
            o.extend_from_slice(s.as_bytes());
        };
        match self {
            Message::Editor { task, active, input_type, ime_options } => {
                o.push(EDITOR);
                i32(&mut o, *task);
                o.push(*active as u8);
                i32(&mut o, *input_type);
                i32(&mut o, *ime_options);
            }
            Message::Compose { text, cursor } => {
                o.push(COMPOSE);
                str(&mut o, text);
                i32(&mut o, *cursor);
            }
            Message::Commit { text } => {
                o.push(COMMIT);
                str(&mut o, text);
            }
            Message::Delete { before, after } => {
                o.push(DELETE);
                i32(&mut o, *before);
                i32(&mut o, *after);
            }
            Message::Action { code } => {
                o.push(ACTION);
                i32(&mut o, *code);
            }
            Message::Key { code } => {
                o.push(KEY);
                i32(&mut o, *code);
            }
        }
        o
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Message> {
        let mut i = In(bytes);
        let m = match i.u8()? {
            EDITOR => Message::Editor { task: i.i32()?, active: i.u8()? != 0, input_type: i.i32()?, ime_options: i.i32()? },
            COMPOSE => Message::Compose { text: i.str()?, cursor: i.i32()? },
            COMMIT => Message::Commit { text: i.str()? },
            DELETE => Message::Delete { before: i.i32()?, after: i.i32()? },
            ACTION => Message::Action { code: i.i32()? },
            KEY => Message::Key { code: i.i32()? },
            _ => return Err(bad()),
        };
        if !i.0.is_empty() {
            return Err(bad());
        }
        Ok(m)
    }

    /// The message as one frame.
    pub fn frame(&self) -> Vec<u8> {
        let body = self.encode();
        let mut out = (body.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&body);
        out
    }

    /// Writes the message as one frame.
    pub fn write(&self, w: &mut impl Write) -> io::Result<()> {
        w.write_all(&self.frame())
    }

    /// Reads one frame; `None` at the end of the stream.
    pub fn read(r: &mut impl Read) -> io::Result<Option<Message>> {
        let mut len = [0u8; 4];
        match r.read_exact(&mut len) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
            Err(e) => return Err(e),
        }
        let len = u32::from_le_bytes(len);
        if len > MAX_FRAME {
            return Err(bad());
        }
        let mut body = vec![0; len as usize];
        r.read_exact(&mut body)?;
        Message::decode(&body).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let messages = [
            Message::Editor { task: 3, active: true, input_type: 0x21, ime_options: 6 },
            Message::Compose { text: "한".into(), cursor: 1 },
            Message::Commit { text: "한글".into() },
            Message::Delete { before: 1, after: 0 },
            Message::Action { code: 6 },
            Message::Key { code: 66 },
        ];
        let mut stream = Vec::new();
        for m in &messages {
            m.write(&mut stream).unwrap();
        }
        let mut r = stream.as_slice();
        for m in &messages {
            assert_eq!(Message::read(&mut r).unwrap().as_ref(), Some(m));
        }
        assert_eq!(Message::read(&mut r).unwrap(), None);
        assert!(Message::decode(&[COMMIT, 9, 0, 0, 0]).is_err());
    }
}
