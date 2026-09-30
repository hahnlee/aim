//! Notifications between the notification bridge (the service host's
//! listener, `aim_services::notifications`), the display server and the
//! window hosts (`docs/notifications.md`).
//!
//! The bridge opens a connection with [`crate::wire::OP_NOTIFICATIONS`];
//! the server answers with its mode (one `u32`), and from then on both
//! ends write [`Message`] frames: a little-endian `u32` length, then the
//! message. Between the server and a window host a frame follows a
//! [`crate::wire::host::NOTIFY`] record.

use std::io::{self, Read, Write};

/// The largest frame either end accepts.
const MAX_FRAME: u32 = 64 << 20;

/// A notification's picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Image {
    /// Rows of RGBA pixels, `width * 4` bytes each.
    Rgba {
        width: u32,
        height: u32,
        premultiplied: bool,
        pixels: Vec<u8>,
    },
    /// An encoded image (PNG, JPEG, WebP, ...).
    Encoded(Vec<u8>),
}

/// One of a notification's actions.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Action {
    pub title: String,
    /// It takes text (a `RemoteInput`): the field's placeholder.
    pub input: Option<String>,
}

/// A posted or updated notification, as the Mac shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Post {
    /// `StatusBarNotification.getKey()`.
    pub key: String,
    pub package: String,
    pub title: String,
    pub subtitle: String,
    pub body: String,
    /// Notifications of one group share it.
    pub thread: String,
    /// Shown without a banner or sound (a low importance, or an update
    /// that alerts only once).
    pub passive: bool,
    /// Counts toward the app's badge.
    pub badge: bool,
    /// It has a full-screen intent SystemUI would launch on a locked or
    /// sleeping device (an incoming call, an alarm).
    pub full_screen: bool,
    pub actions: Vec<Action>,
    pub image: Option<Image>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Bridge to host: show or update a notification.
    Post(Post),
    /// Bridge to host: the notification is gone.
    Remove { key: String, package: String },
    /// Host to bridge: the user clicked it.
    Click { key: String },
    /// Host to bridge: the user chose action `index`, with text if it
    /// takes some.
    Action {
        key: String,
        index: u32,
        reply: Option<String>,
    },
    /// Host to bridge: the user dismissed it.
    Dismiss { key: String },
    /// Server to bridge: the Mac is locked or asleep; launch the
    /// notification's full-screen intent.
    FullScreen { key: String },
    /// Host to server: after a post or removal, whether the Mac shows the
    /// notification, or why it could not (the server logs it).
    Shown {
        key: String,
        shown: bool,
        error: Option<String>,
    },
    /// Bridge to host: the app asks for POST_NOTIFICATIONS; ask the Mac
    /// for `package`'s full authorization (its prompt, the first time).
    Authorize { package: String },
    /// Host to bridge: the Mac's setting for `package`, allowed (fully,
    /// provisionally or for a session), denied, or not yet determined
    /// (`None`). A shim sends it when it starts, when its app becomes
    /// active, when the setting changed as it posted, and in answer to
    /// [`Message::Authorize`] (`answer`).
    Authorization {
        package: String,
        allowed: Option<bool>,
        answer: bool,
    },
}

const POST: u8 = 1;
const REMOVE: u8 = 2;
const CLICK: u8 = 3;
const ACTION: u8 = 4;
const DISMISS: u8 = 5;
const SHOWN: u8 = 6;
const FULL_SCREEN: u8 = 7;
const AUTHORIZE: u8 = 8;
const AUTHORIZATION: u8 = 9;

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
    }
    fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
    }
    fn opt(&mut self, v: Option<&str>) {
        self.u8(v.is_some() as u8);
        if let Some(v) = v {
            self.str(v);
        }
    }
}

struct In<'a>(&'a [u8]);

fn bad() -> io::Error {
    io::ErrorKind::InvalidData.into()
}

impl In<'_> {
    fn take(&mut self, n: usize) -> io::Result<&[u8]> {
        if self.0.len() < n {
            return Err(bad());
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> io::Result<String> {
        String::from_utf8(self.bytes()?).map_err(|_| bad())
    }
    fn opt(&mut self) -> io::Result<Option<String>> {
        Ok(if self.u8()? != 0 {
            Some(self.str()?)
        } else {
            None
        })
    }
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Out(Vec::new());
        match self {
            Message::Post(p) => {
                o.u8(POST);
                for s in [
                    &p.key,
                    &p.package,
                    &p.title,
                    &p.subtitle,
                    &p.body,
                    &p.thread,
                ] {
                    o.str(s);
                }
                o.u8(p.passive as u8 | (p.badge as u8) << 1 | (p.full_screen as u8) << 2);
                o.u32(p.actions.len() as u32);
                for a in &p.actions {
                    o.str(&a.title);
                    o.opt(a.input.as_deref());
                }
                match &p.image {
                    None => o.u8(0),
                    Some(Image::Rgba {
                        width,
                        height,
                        premultiplied,
                        pixels,
                    }) => {
                        o.u8(1);
                        o.u32(*width);
                        o.u32(*height);
                        o.u8(*premultiplied as u8);
                        o.bytes(pixels);
                    }
                    Some(Image::Encoded(data)) => {
                        o.u8(2);
                        o.bytes(data);
                    }
                }
            }
            Message::Remove { key, package } => {
                o.u8(REMOVE);
                o.str(key);
                o.str(package);
            }
            Message::Click { key } => {
                o.u8(CLICK);
                o.str(key);
            }
            Message::Action { key, index, reply } => {
                o.u8(ACTION);
                o.str(key);
                o.u32(*index);
                o.opt(reply.as_deref());
            }
            Message::Dismiss { key } => {
                o.u8(DISMISS);
                o.str(key);
            }
            Message::FullScreen { key } => {
                o.u8(FULL_SCREEN);
                o.str(key);
            }
            Message::Shown { key, shown, error } => {
                o.u8(SHOWN);
                o.str(key);
                o.u8(*shown as u8);
                o.opt(error.as_deref());
            }
            Message::Authorize { package } => {
                o.u8(AUTHORIZE);
                o.str(package);
            }
            Message::Authorization {
                package,
                allowed,
                answer,
            } => {
                o.u8(AUTHORIZATION);
                o.str(package);
                o.u8(match allowed {
                    None => 0,
                    Some(false) => 1,
                    Some(true) => 2,
                });
                o.u8(*answer as u8);
            }
        }
        o.0
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Message> {
        let mut i = In(bytes);
        let m = match i.u8()? {
            POST => {
                let (key, package, title) = (i.str()?, i.str()?, i.str()?);
                let (subtitle, body, thread) = (i.str()?, i.str()?, i.str()?);
                let flags = i.u8()?;
                let mut actions = Vec::new();
                for _ in 0..i.u32()? {
                    actions.push(Action {
                        title: i.str()?,
                        input: i.opt()?,
                    });
                }
                let image = match i.u8()? {
                    0 => None,
                    1 => {
                        let (width, height) = (i.u32()?, i.u32()?);
                        let premultiplied = i.u8()? != 0;
                        let pixels = i.bytes()?;
                        if pixels.len() as u64 != u64::from(width) * u64::from(height) * 4 {
                            return Err(bad());
                        }
                        Some(Image::Rgba {
                            width,
                            height,
                            premultiplied,
                            pixels,
                        })
                    }
                    2 => Some(Image::Encoded(i.bytes()?)),
                    _ => return Err(bad()),
                };
                Message::Post(Post {
                    key,
                    package,
                    title,
                    subtitle,
                    body,
                    thread,
                    passive: flags & 1 != 0,
                    badge: flags & 2 != 0,
                    full_screen: flags & 4 != 0,
                    actions,
                    image,
                })
            }
            REMOVE => Message::Remove {
                key: i.str()?,
                package: i.str()?,
            },
            CLICK => Message::Click { key: i.str()? },
            ACTION => Message::Action {
                key: i.str()?,
                index: i.u32()?,
                reply: i.opt()?,
            },
            DISMISS => Message::Dismiss { key: i.str()? },
            FULL_SCREEN => Message::FullScreen { key: i.str()? },
            SHOWN => Message::Shown {
                key: i.str()?,
                shown: i.u8()? != 0,
                error: i.opt()?,
            },
            AUTHORIZE => Message::Authorize { package: i.str()? },
            AUTHORIZATION => Message::Authorization {
                package: i.str()?,
                allowed: match i.u8()? {
                    0 => None,
                    1 => Some(false),
                    2 => Some(true),
                    _ => return Err(bad()),
                },
                answer: i.u8()? != 0,
            },
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

    /// The key of the notification it is about, or the package.
    pub fn key(&self) -> &str {
        match self {
            Message::Post(p) => &p.key,
            Message::Remove { key, .. }
            | Message::Click { key }
            | Message::Action { key, .. }
            | Message::Dismiss { key }
            | Message::FullScreen { key }
            | Message::Shown { key, .. } => key,
            Message::Authorize { package } | Message::Authorization { package, .. } => package,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let messages = [
            Message::Post(Post {
                key: "0|com.example|1|null|10123".into(),
                package: "com.example".into(),
                title: "Title".into(),
                subtitle: String::new(),
                body: "Hello\nworld".into(),
                thread: "g".into(),
                passive: true,
                badge: true,
                full_screen: true,
                actions: vec![
                    Action {
                        title: "Reply".into(),
                        input: Some("Message".into()),
                    },
                    Action {
                        title: "Mark read".into(),
                        input: None,
                    },
                ],
                image: Some(Image::Rgba {
                    width: 1,
                    height: 2,
                    premultiplied: true,
                    pixels: vec![1, 2, 3, 4, 5, 6, 7, 8],
                }),
            }),
            Message::Remove {
                key: "k".into(),
                package: "p".into(),
            },
            Message::Click { key: "k".into() },
            Message::Action {
                key: "k".into(),
                index: 1,
                reply: Some("ok".into()),
            },
            Message::Dismiss { key: "k".into() },
            Message::FullScreen { key: "k".into() },
            Message::Shown {
                key: "k".into(),
                shown: false,
                error: Some("denied".into()),
            },
            Message::Authorize {
                package: "p".into(),
            },
            Message::Authorization {
                package: "p".into(),
                allowed: None,
                answer: true,
            },
            Message::Authorization {
                package: "p".into(),
                allowed: Some(false),
                answer: false,
            },
            Message::Authorization {
                package: "p".into(),
                allowed: Some(true),
                answer: true,
            },
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
    }

    #[test]
    fn refuses_a_short_image() {
        let m = Message::Post(Post {
            image: Some(Image::Rgba {
                width: 2,
                height: 2,
                premultiplied: false,
                pixels: vec![0; 4],
            }),
            ..Default::default()
        });
        assert!(Message::decode(&m.encode()).is_err());
    }
}
