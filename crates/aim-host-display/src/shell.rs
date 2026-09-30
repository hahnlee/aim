//! The lightweight shell's status bar between the service host's native
//! `IStatusBar` (`aim_services::statusbar`), the display server and the
//! window hosts (`docs/m1-shell.md`, "What else SystemUI provides"): an
//! app's text toasts, its system status icons, the device credential
//! sheet of `BiometricPrompt` and the screen pinning request, each shown
//! by the app's shim.
//!
//! The status bar opens a connection with [`crate::wire::OP_SHELL`]; the
//! server answers with its mode (one `u32`), and from then on both ends
//! write [`Message`] frames: a little-endian `u32` length, then the
//! message. Between the server and a window host a frame follows a
//! [`crate::wire::host::SHELL`] record.

use std::io::{self, Read, Write};

use crate::notify::Image;

/// The largest frame either end accepts.
const MAX_FRAME: u32 = 16 << 20;

/// A system status icon (`IStatusBar.setIcon`), as the app's menu bar item
/// shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusIcon {
    /// The icon, whose alpha the item draws (a template image).
    pub image: Option<Image>,
    pub level: i32,
    pub number: i32,
    /// Its `contentDescription`, for the item's accessibility label.
    pub description: String,
    pub visible: bool,
}

/// The kind of the user's device credential (`LockPatternUtils`'s
/// credential types).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Credential {
    Pin,
    Password,
    /// Entered as the numbers of its dots, 1 to 9 row by row, as
    /// `LockPatternUtils.patternToByteArray` has them.
    Pattern,
}

/// A request for the device credential (`IStatusBar.showAuthenticationDialog`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authenticate {
    /// BiometricService's request id.
    pub id: u64,
    pub package: String,
    pub title: String,
    pub subtitle: String,
    pub description: String,
    pub credential: Credential,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// Status bar to host: show text toast `id` of `package`.
    Toast {
        id: u32,
        package: String,
        text: String,
    },
    /// Status bar to host: hide it (NotificationManagerService's timing, or
    /// a next toast replacing it).
    HideToast { id: u32, package: String },
    /// Host to status bar: the toast is on screen.
    ToastShown { id: u32 },
    /// Host to status bar: the toast is gone; `shown` if it had been on
    /// screen.
    ToastHidden { id: u32, shown: bool },
    /// Status bar to host: `package`'s icon in `slot`; `None` removes it.
    Icon {
        package: String,
        slot: String,
        icon: Option<StatusIcon>,
    },
    /// Status bar to host: ask for the device credential.
    Authenticate(Authenticate),
    /// Host to status bar: what the user entered.
    Secret { id: u64, secret: String },
    /// Status bar to host: the secret was wrong; no new one is taken for
    /// `lockout_ms`.
    Retry {
        id: u64,
        package: String,
        lockout_ms: u32,
    },
    /// Host to status bar: the user cancelled, or (`shown` false) the
    /// sheet could not be shown.
    Cancel { id: u64, shown: bool },
    /// Status bar to host: close the sheet (verified, or cancelled by
    /// BiometricService).
    Dismiss { id: u64, package: String },
    /// Status bar to host: ask whether to pin task `task`.
    Pin { task: i32 },
    /// Host to status bar: the user's answer (false as well when it could
    /// not be asked).
    Pinned { task: i32, accepted: bool },
}

const TOAST: u8 = 1;
const HIDE_TOAST: u8 = 2;
const TOAST_SHOWN: u8 = 3;
const TOAST_HIDDEN: u8 = 4;
const ICON: u8 = 5;
const AUTHENTICATE: u8 = 6;
const SECRET: u8 = 7;
const RETRY: u8 = 8;
const CANCEL: u8 = 9;
const DISMISS: u8 = 10;
const PIN: u8 = 11;
const PINNED: u8 = 12;

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, v: &[u8]) {
        self.u32(v.len() as u32);
        self.0.extend_from_slice(v);
    }
    fn str(&mut self, v: &str) {
        self.bytes(v.as_bytes());
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
    fn u64(&mut self) -> io::Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn bytes(&mut self) -> io::Result<Vec<u8>> {
        let n = self.u32()? as usize;
        Ok(self.take(n)?.to_vec())
    }
    fn str(&mut self) -> io::Result<String> {
        String::from_utf8(self.bytes()?).map_err(|_| bad())
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
            Message::Toast { id, package, text } => {
                o.u8(TOAST);
                o.u32(*id);
                o.str(package);
                o.str(text);
            }
            Message::HideToast { id, package } => {
                o.u8(HIDE_TOAST);
                o.u32(*id);
                o.str(package);
            }
            Message::ToastShown { id } => {
                o.u8(TOAST_SHOWN);
                o.u32(*id);
            }
            Message::ToastHidden { id, shown } => {
                o.u8(TOAST_HIDDEN);
                o.u32(*id);
                o.u8(*shown as u8);
            }
            Message::Icon {
                package,
                slot,
                icon,
            } => {
                o.u8(ICON);
                o.str(package);
                o.str(slot);
                o.u8(icon.is_some() as u8);
                if let Some(i) = icon {
                    o.image(i.image.as_ref());
                    o.u32(i.level as u32);
                    o.u32(i.number as u32);
                    o.str(&i.description);
                    o.u8(i.visible as u8);
                }
            }
            Message::Authenticate(a) => {
                o.u8(AUTHENTICATE);
                o.u64(a.id);
                for s in [&a.package, &a.title, &a.subtitle, &a.description] {
                    o.str(s);
                }
                o.u8(match a.credential {
                    Credential::Pin => 0,
                    Credential::Password => 1,
                    Credential::Pattern => 2,
                });
            }
            Message::Secret { id, secret } => {
                o.u8(SECRET);
                o.u64(*id);
                o.str(secret);
            }
            Message::Retry {
                id,
                package,
                lockout_ms,
            } => {
                o.u8(RETRY);
                o.u64(*id);
                o.str(package);
                o.u32(*lockout_ms);
            }
            Message::Cancel { id, shown } => {
                o.u8(CANCEL);
                o.u64(*id);
                o.u8(*shown as u8);
            }
            Message::Dismiss { id, package } => {
                o.u8(DISMISS);
                o.u64(*id);
                o.str(package);
            }
            Message::Pin { task } => {
                o.u8(PIN);
                o.u32(*task as u32);
            }
            Message::Pinned { task, accepted } => {
                o.u8(PINNED);
                o.u32(*task as u32);
                o.u8(*accepted as u8);
            }
        }
        o.0
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Message> {
        let mut i = In(bytes);
        let m = match i.u8()? {
            TOAST => Message::Toast {
                id: i.u32()?,
                package: i.str()?,
                text: i.str()?,
            },
            HIDE_TOAST => Message::HideToast {
                id: i.u32()?,
                package: i.str()?,
            },
            TOAST_SHOWN => Message::ToastShown { id: i.u32()? },
            TOAST_HIDDEN => Message::ToastHidden {
                id: i.u32()?,
                shown: i.u8()? != 0,
            },
            ICON => Message::Icon {
                package: i.str()?,
                slot: i.str()?,
                icon: if i.u8()? != 0 {
                    Some(StatusIcon {
                        image: i.image()?,
                        level: i.u32()? as i32,
                        number: i.u32()? as i32,
                        description: i.str()?,
                        visible: i.u8()? != 0,
                    })
                } else {
                    None
                },
            },
            AUTHENTICATE => Message::Authenticate(Authenticate {
                id: i.u64()?,
                package: i.str()?,
                title: i.str()?,
                subtitle: i.str()?,
                description: i.str()?,
                credential: match i.u8()? {
                    0 => Credential::Pin,
                    1 => Credential::Password,
                    2 => Credential::Pattern,
                    _ => return Err(bad()),
                },
            }),
            SECRET => Message::Secret {
                id: i.u64()?,
                secret: i.str()?,
            },
            RETRY => Message::Retry {
                id: i.u64()?,
                package: i.str()?,
                lockout_ms: i.u32()?,
            },
            CANCEL => Message::Cancel {
                id: i.u64()?,
                shown: i.u8()? != 0,
            },
            DISMISS => Message::Dismiss {
                id: i.u64()?,
                package: i.str()?,
            },
            PIN => Message::Pin {
                task: i.u32()? as i32,
            },
            PINNED => Message::Pinned {
                task: i.u32()? as i32,
                accepted: i.u8()? != 0,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let messages = [
            Message::Toast {
                id: 7,
                package: "com.example".into(),
                text: "Saved".into(),
            },
            Message::HideToast {
                id: 7,
                package: "com.example".into(),
            },
            Message::ToastShown { id: 7 },
            Message::ToastHidden { id: 7, shown: true },
            Message::Icon {
                package: "android".into(),
                slot: "ime".into(),
                icon: Some(StatusIcon {
                    image: Some(Image::Rgba {
                        width: 1,
                        height: 1,
                        premultiplied: true,
                        pixels: vec![1, 2, 3, 4],
                    }),
                    level: 1,
                    number: -1,
                    description: "Keyboard".into(),
                    visible: true,
                }),
            },
            Message::Icon {
                package: "android".into(),
                slot: "ime".into(),
                icon: None,
            },
            Message::Authenticate(Authenticate {
                id: 1 << 40,
                package: "com.example".into(),
                title: "Sign in".into(),
                subtitle: String::new(),
                description: "d".into(),
                credential: Credential::Pattern,
            }),
            Message::Secret {
                id: 3,
                secret: "1234".into(),
            },
            Message::Retry {
                id: 3,
                package: "p".into(),
                lockout_ms: 30_000,
            },
            Message::Cancel {
                id: 3,
                shown: false,
            },
            Message::Dismiss {
                id: 3,
                package: "p".into(),
            },
            Message::Pin { task: 12 },
            Message::Pinned {
                task: 12,
                accepted: true,
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
}
