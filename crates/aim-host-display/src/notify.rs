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

/// Where a live notification's chronometer stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clock {
    /// It counts up from, or down to, `base_ms` of the guest's
    /// `SystemClock.elapsedRealtime()` (`CLOCK_BOOTTIME`,
    /// `aim_hostcall::clock::boottime_ns`).
    Counting { base_ms: i64, down: bool },
    /// It stands still at `shown_ms` (a paused timer).
    Stopped { shown_ms: i64 },
}

impl Clock {
    /// What an Android `Chronometer` shows at `now_ms` of the same clock
    /// (`Chronometer.updateText`, `DateUtils.formatElapsedTime`).
    pub fn text(&self, now_ms: i64) -> String {
        let ms = match *self {
            Clock::Counting { base_ms, down } if down => base_ms - now_ms,
            Clock::Counting { base_ms, .. } => now_ms - base_ms,
            Clock::Stopped { shown_ms } => shown_ms,
        };
        let s = (ms / 1000).unsigned_abs();
        let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
        let text = if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m:02}:{s:02}")
        };
        if ms / 1000 < 0 {
            format!("\u{2212}{text}")
        } else {
            text
        }
    }

    /// How long after `now_ms` its text may next change: at the next
    /// whole second it counts; never when it stands still.
    pub fn next_change_ms(&self, now_ms: i64) -> Option<i64> {
        let Clock::Counting { base_ms, .. } = *self else {
            return None;
        };
        match (now_ms - base_ms).rem_euclid(1000) {
            0 => Some(1000),
            r => Some(1000 - r),
        }
    }
}

/// A live notification's progress.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub value: u32,
    pub max: u32,
    pub indeterminate: bool,
}

/// An ongoing activity the app's menu bar item shows while its
/// notification lasts, as SystemUI's status bar chip shows an Android 16
/// Live Update (`docs/notifications.md`, "The menu bar").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Live {
    /// `Notification.EXTRA_SHORT_CRITICAL_TEXT`, if the app gave one.
    pub text: String,
    pub clock: Option<Clock>,
    pub progress: Option<Progress>,
    /// The small icon, whose alpha the item draws (a template image).
    pub icon: Option<Image>,
}

/// What an app's indicator item stands for: an app op SystemUI's status
/// bar indicators watch while the app has it active
/// (`AppOpsManager.startWatchingActive`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Indicator {
    Microphone,
    Camera,
    Location,
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
    /// It is an ongoing activity the menu bar shows.
    pub live: Option<Box<Live>>,
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
    /// Bridge to host: `package` has, or no longer has, what `indicator`
    /// stands for in use.
    Indicator {
        package: String,
        indicator: Indicator,
        on: bool,
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
const INDICATOR: u8 = 10;

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i64(&mut self, v: i64) {
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
    fn image(&mut self, v: Option<&Image>) {
        match v {
            None => self.u8(0),
            Some(Image::Rgba {
                width,
                height,
                premultiplied,
                pixels,
            }) => {
                self.u8(1);
                self.u32(*width);
                self.u32(*height);
                self.u8(*premultiplied as u8);
                self.bytes(pixels);
            }
            Some(Image::Encoded(data)) => {
                self.u8(2);
                self.bytes(data);
            }
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
    fn i64(&mut self) -> io::Result<i64> {
        Ok(i64::from_le_bytes(self.take(8)?.try_into().unwrap()))
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
    fn image(&mut self) -> io::Result<Option<Image>> {
        Ok(match self.u8()? {
            0 => None,
            1 => {
                let (width, height) = (self.u32()?, self.u32()?);
                let premultiplied = self.u8()? != 0;
                let pixels = self.bytes()?;
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
            2 => Some(Image::Encoded(self.bytes()?)),
            _ => return Err(bad()),
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
                o.image(p.image.as_ref());
                o.u8(p.live.is_some() as u8);
                if let Some(l) = &p.live {
                    o.str(&l.text);
                    match l.clock {
                        None => o.u8(0),
                        Some(Clock::Counting { base_ms, down }) => {
                            o.u8(1);
                            o.i64(base_ms);
                            o.u8(down as u8);
                        }
                        Some(Clock::Stopped { shown_ms }) => {
                            o.u8(2);
                            o.i64(shown_ms);
                        }
                    }
                    o.u8(l.progress.is_some() as u8);
                    if let Some(p) = l.progress {
                        o.u32(p.value);
                        o.u32(p.max);
                        o.u8(p.indeterminate as u8);
                    }
                    o.image(l.icon.as_ref());
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
            Message::Indicator {
                package,
                indicator,
                on,
            } => {
                o.u8(INDICATOR);
                o.str(package);
                o.u8(*indicator as u8);
                o.u8(*on as u8);
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
                let image = i.image()?;
                let live = if i.u8()? != 0 {
                    let text = i.str()?;
                    let clock = match i.u8()? {
                        0 => None,
                        1 => Some(Clock::Counting {
                            base_ms: i.i64()?,
                            down: i.u8()? != 0,
                        }),
                        2 => Some(Clock::Stopped { shown_ms: i.i64()? }),
                        _ => return Err(bad()),
                    };
                    let progress = if i.u8()? != 0 {
                        Some(Progress {
                            value: i.u32()?,
                            max: i.u32()?,
                            indeterminate: i.u8()? != 0,
                        })
                    } else {
                        None
                    };
                    Some(Box::new(Live {
                        text,
                        clock,
                        progress,
                        icon: i.image()?,
                    }))
                } else {
                    None
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
                    live,
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
            INDICATOR => Message::Indicator {
                package: i.str()?,
                indicator: match i.u8()? {
                    0 => Indicator::Microphone,
                    1 => Indicator::Camera,
                    2 => Indicator::Location,
                    _ => return Err(bad()),
                },
                on: i.u8()? != 0,
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
            Message::Authorize { package }
            | Message::Authorization { package, .. }
            | Message::Indicator { package, .. } => package,
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
                live: Some(Box::new(Live {
                    text: "5 min".into(),
                    clock: Some(Clock::Counting {
                        base_ms: 1_234_567,
                        down: true,
                    }),
                    progress: Some(Progress {
                        value: 3,
                        max: 10,
                        indeterminate: false,
                    }),
                    icon: Some(Image::Encoded(vec![0x89, b'P'])),
                })),
            }),
            Message::Post(Post {
                key: "k".into(),
                live: Some(Box::new(Live {
                    clock: Some(Clock::Stopped { shown_ms: -5_000 }),
                    ..Default::default()
                })),
                ..Default::default()
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
            Message::Indicator {
                package: "p".into(),
                indicator: Indicator::Location,
                on: true,
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
    fn reads_as_a_chronometer() {
        let down = Clock::Counting {
            base_ms: 300_000,
            down: true,
        };
        assert_eq!(down.text(1_000), "04:59");
        assert_eq!(down.text(300_999), "00:00");
        assert_eq!(down.text(305_000), "\u{2212}00:05");
        let up = Clock::Counting {
            base_ms: 0,
            down: false,
        };
        assert_eq!(up.text(3_723_000), "1:02:03");
        assert_eq!(Clock::Stopped { shown_ms: 59_999 }.text(0), "00:59");
        assert_eq!(down.next_change_ms(1_000), Some(1000));
        assert_eq!(down.next_change_ms(1_250), Some(750));
        assert_eq!(up.next_change_ms(-1_250), Some(250));
        assert_eq!(Clock::Stopped { shown_ms: 0 }.next_change_ms(0), None);
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
