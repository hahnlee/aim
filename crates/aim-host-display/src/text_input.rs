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
    Editor {
        task: i32,
        active: bool,
        input_type: i32,
        ime_options: i32,
    },
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
    /// Host to app host: the marked text is taken as it is
    /// (`finishComposingText`).
    Finish,
}

const EDITOR: u8 = 1;
const COMPOSE: u8 = 2;
const COMMIT: u8 = 3;
const DELETE: u8 = 4;
const ACTION: u8 = 5;
const KEY: u8 = 6;
const FINISH: u8 = 7;

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
            Message::Editor {
                task,
                active,
                input_type,
                ime_options,
            } => {
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
            Message::Finish => o.push(FINISH),
        }
        o
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Message> {
        let mut i = In(bytes);
        let m = match i.u8()? {
            EDITOR => Message::Editor {
                task: i.i32()?,
                active: i.u8()? != 0,
                input_type: i.i32()?,
                ime_options: i.i32()?,
            },
            COMPOSE => Message::Compose {
                text: i.str()?,
                cursor: i.i32()?,
            },
            COMMIT => Message::Commit { text: i.str()? },
            DELETE => Message::Delete {
                before: i.i32()?,
                after: i.i32()?,
            },
            ACTION => Message::Action { code: i.i32()? },
            KEY => Message::Key { code: i.i32()? },
            FINISH => Message::Finish,
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

/// `NSNotFound`: no range.
pub const NOT_FOUND: usize = isize::MAX as usize;

/// What the Mac's input method composes in an editor, as its client (the
/// window's `NSTextInputClient`) keeps it: the text the input method put
/// before the caret since the editor started, the caret last moved or the
/// view was touched (what it reads back and replaces: Korean input
/// composes a syllable by replacing the one before the caret), and the
/// marked text after it. The editor's other text is the app's. Each
/// change gives the [`Message`]s that make it on the editor, in UTF-16
/// units as `NSRange` and `InputConnection` count.
#[derive(Default)]
pub struct Composition {
    typed: String,
    marked: String,
    /// Whether the input method's answer to the discard of the old
    /// editor's marked text may still come: until its next key.
    discarding: bool,
}

impl Composition {
    pub const fn new() -> Self {
        Composition {
            typed: String::new(),
            marked: String::new(),
            discarding: false,
        }
    }

    /// A new editor, or none: nothing is typed or marked. Whether there
    /// was marked text, for the input method to discard.
    ///
    /// The app has already finished that text in the old editor. The input
    /// method answers the discard with its own commit or marked text, in
    /// the call or later from its process, but before it takes the next
    /// key: what it gives until then is the old editor's and changes
    /// nothing.
    pub fn reset(&mut self) -> bool {
        self.typed.clear();
        let marked = !std::mem::take(&mut self.marked).is_empty();
        self.discarding |= marked;
        marked
    }

    /// The input method takes a key: what it gives now is this editor's.
    pub fn key(&mut self) {
        self.discarding = false;
    }

    /// The caret moved where the input method does not know: what it
    /// typed is no longer before it.
    pub fn forget_typed(&mut self) {
        self.typed.clear();
    }

    /// The typed text and the marked text after it.
    pub fn document(&self) -> Vec<u16> {
        self.typed
            .encode_utf16()
            .chain(self.marked.encode_utf16())
            .collect()
    }

    /// The marked text's range (`markedRange`), if there is marked text.
    pub fn marked_range(&self) -> Option<(usize, usize)> {
        (!self.marked.is_empty()).then(|| {
            (
                self.typed.encode_utf16().count(),
                self.marked.encode_utf16().count(),
            )
        })
    }

    /// A replacement of the typed text from `from` on: the app's text
    /// before the caret goes as far back (`deleteSurroundingText`), and so
    /// does the typed text.
    fn replace(&mut self, from: usize, out: &mut Vec<Message>) {
        if from == NOT_FOUND {
            return;
        }
        let units: Vec<u16> = self.typed.encode_utf16().collect();
        if from < units.len() {
            out.push(Message::Delete {
                before: (units.len() - from) as i32,
                after: 0,
            });
            self.typed = String::from_utf16_lossy(&units[..from]);
        }
    }

    /// `insertText:replacementRange:`, the replacement from `from`.
    pub fn insert(&mut self, text: String, from: usize) -> Vec<Message> {
        let mut out = Vec::new();
        if self.discarding {
            return out;
        }
        self.replace(from, &mut out);
        self.marked.clear();
        self.typed.push_str(&text);
        out.push(Message::Commit { text });
        out
    }

    /// `setMarkedText:selectedRange:replacementRange:`, the replacement
    /// from `from`; the cursor after the marked text, as the input method
    /// keeps it.
    pub fn mark(&mut self, text: String, from: usize) -> Vec<Message> {
        let mut out = Vec::new();
        if self.discarding {
            return out;
        }
        self.replace(from, &mut out);
        self.marked = text.clone();
        out.push(Message::Compose { text, cursor: 1 });
        out
    }

    /// `unmarkText`: the marked text is taken as it is.
    pub fn unmark(&mut self) -> Option<Message> {
        if self.discarding {
            return None;
        }
        let text = std::mem::take(&mut self.marked);
        (!text.is_empty()).then(|| {
            self.typed.push_str(&text);
            Message::Finish
        })
    }

    /// `deleteBackward:`: the character before the caret.
    pub fn delete_backward(&mut self) -> Message {
        let before = self.typed.pop().map_or(1, char::len_utf16);
        Message::Delete {
            before: before as i32,
            after: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let messages = [
            Message::Editor {
                task: 3,
                active: true,
                input_type: 0x21,
                ime_options: 6,
            },
            Message::Compose {
                text: "한".into(),
                cursor: 1,
            },
            Message::Commit {
                text: "한글".into(),
            },
            Message::Delete {
                before: 1,
                after: 0,
            },
            Message::Action { code: 6 },
            Message::Key { code: 66 },
            Message::Finish,
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

    fn commit(text: &str) -> Message {
        Message::Commit { text: text.into() }
    }

    fn compose(text: &str) -> Message {
        Message::Compose {
            text: text.into(),
            cursor: 1,
        }
    }

    /// Korean input as marked text: a syllable is marked until the next
    /// one starts, then inserted, and the last one taken as it is.
    #[test]
    fn marked_hangul_composes_and_commits() {
        let mut c = Composition::new();
        assert_eq!(c.mark("ㅎ".into(), NOT_FOUND), [compose("ㅎ")]);
        assert_eq!(c.mark("하".into(), NOT_FOUND), [compose("하")]);
        assert_eq!(c.mark("한".into(), NOT_FOUND), [compose("한")]);
        assert_eq!(c.marked_range(), Some((0, 1)));
        assert_eq!(c.insert("한".into(), NOT_FOUND), [commit("한")]);
        assert_eq!(c.mark("ㄱ".into(), NOT_FOUND), [compose("ㄱ")]);
        assert_eq!(c.mark("글".into(), NOT_FOUND), [compose("글")]);
        assert_eq!(c.marked_range(), Some((1, 1)));
        assert_eq!(c.unmark(), Some(Message::Finish));
        assert_eq!(c.unmark(), None);
        assert_eq!(c.marked_range(), None);
        assert_eq!(c.document(), "한글".encode_utf16().collect::<Vec<_>>());
    }

    /// Korean input by replacement: each key inserts the syllable again
    /// over the one before the caret, as the input method reads it back.
    #[test]
    fn replaced_hangul_deletes_what_it_replaces() {
        let mut c = Composition::new();
        assert_eq!(c.insert("ㅎ".into(), NOT_FOUND), [commit("ㅎ")]);
        let replace = |c: &mut Composition, s: &str| c.insert(s.into(), 0);
        let delete = Message::Delete {
            before: 1,
            after: 0,
        };
        assert_eq!(replace(&mut c, "하"), [delete.clone(), commit("하")]);
        assert_eq!(replace(&mut c, "한"), [delete.clone(), commit("한")]);
        // Backspace takes the syllable apart: what is left replaces it.
        assert_eq!(replace(&mut c, "하"), [delete.clone(), commit("하")]);
        assert_eq!(c.document(), "하".encode_utf16().collect::<Vec<_>>());
        assert_eq!(c.delete_backward(), delete);
        assert_eq!(c.document(), []);
    }

    /// The editor's text before what the input method typed is the app's:
    /// a replacement never reaches back into it, and a new editor or a
    /// moved caret starts over.
    #[test]
    fn replacement_stays_within_the_typed_text() {
        let mut c = Composition::new();
        c.insert("😀a".into(), NOT_FOUND);
        assert_eq!(
            c.delete_backward(),
            Message::Delete {
                before: 1,
                after: 0
            }
        );
        assert_eq!(
            c.delete_backward(),
            Message::Delete {
                before: 2,
                after: 0
            }
        );
        c.mark("ㅎ".into(), NOT_FOUND);
        assert!(c.reset());
        assert!(!c.reset());
        c.key();
        c.insert("ab".into(), NOT_FOUND);
        c.forget_typed();
        assert_eq!(c.insert("c".into(), 0), [commit("c")]);
    }

    /// The input method's answer to the discard of the old editor's
    /// marked text, in the call or after it, is the old editor's: the new
    /// editor gets none of it, and a replacement in the new editor never
    /// deletes the app's text for it.
    #[test]
    fn discarded_text_stays_with_the_old_editor() {
        let mut c = Composition::new();
        c.mark("한".into(), NOT_FOUND);
        assert!(c.reset());
        assert_eq!(c.insert("한".into(), NOT_FOUND), []);
        assert_eq!(c.mark("한".into(), NOT_FOUND), []);
        assert_eq!(c.unmark(), None);
        assert_eq!(c.marked_range(), None);
        assert_eq!(c.document(), []);
        // A start without marked text leaves the discard pending.
        assert!(!c.reset());
        assert_eq!(c.insert("한".into(), NOT_FOUND), []);
        c.key();
        assert_eq!(c.insert("ㄱ".into(), NOT_FOUND), [commit("ㄱ")]);
        assert_eq!(
            c.insert("가".into(), 0),
            [
                Message::Delete {
                    before: 1,
                    after: 0
                },
                commit("가")
            ]
        );
        assert_eq!(
            c.insert("나".into(), 0),
            [
                Message::Delete {
                    before: 1,
                    after: 0
                },
                commit("나")
            ]
        );
        assert_eq!(c.document(), "나".encode_utf16().collect::<Vec<_>>());
    }
}
