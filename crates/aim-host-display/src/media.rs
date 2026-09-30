//! Media between the media bridge (the service host's side,
//! `aim_services::media`), the display server and the window hosts
//! (`docs/media.md`): the playing media session as the Mac's Now Playing,
//! the Mac's transport commands for it, and the Mac's consent to an app's
//! screen capture.
//!
//! The bridge opens a connection with [`crate::wire::OP_MEDIA`]; the
//! server answers with its mode (one `u32`), and from then on both ends
//! write [`Message`] frames: a little-endian `u32` length, then the
//! message. Between the server and a window host a frame follows a
//! [`crate::wire::host::MEDIA`] record.

use std::io::{self, Read, Write};

use crate::notify::Image;

/// The largest frame either end accepts.
const MAX_FRAME: u32 = 64 << 20;

/// What a session is doing, as the Mac's `MPNowPlayingPlaybackState` has it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum State {
    #[default]
    Unknown,
    Playing,
    Paused,
    Stopped,
    /// Buffering, connecting, skipping: it will play.
    Interrupted,
}

/// A transport command, from the Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Play,
    Pause,
    TogglePlayPause,
    Stop,
    Next,
    Previous,
    /// To this position, in milliseconds.
    Seek(u64),
}

/// [`NowPlaying::commands`]: the commands the session takes.
pub mod commands {
    pub const PLAY: u32 = 1 << 0;
    pub const PAUSE: u32 = 1 << 1;
    pub const STOP: u32 = 1 << 2;
    pub const NEXT: u32 = 1 << 3;
    pub const PREVIOUS: u32 = 1 << 4;
    pub const SEEK: u32 = 1 << 5;
}

/// The session the Mac's media keys control, as Now Playing shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowPlaying {
    pub package: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// In milliseconds, if known.
    pub duration_ms: Option<u64>,
    /// The position when the message was sent, in milliseconds.
    pub position_ms: u64,
    /// How fast the position advances (1 while playing at normal speed).
    pub rate: f32,
    pub state: State,
    /// Of [`commands`].
    pub commands: u32,
    pub artwork: Option<Image>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Message {
    /// Bridge to host: show `package`'s session as Now Playing, or update
    /// it.
    NowPlaying(NowPlaying),
    /// Bridge to host: `package` no longer has the session.
    Gone { package: String },
    /// Host to bridge: the user's command for `package`'s session.
    Command { package: String, command: Command },
    /// Bridge to host: ask the user whether `package` (labelled `label`)
    /// may capture the screen.
    Consent {
        id: u32,
        package: String,
        label: String,
    },
    /// Host to bridge: the user's answer to [`Message::Consent`] `id`.
    Consented { id: u32, allowed: bool },
}

const NOW_PLAYING: u8 = 1;
const GONE: u8 = 2;
const COMMAND: u8 = 3;
const CONSENT: u8 = 4;
const CONSENTED: u8 = 5;

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
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let mut o = Out(Vec::new());
        match self {
            Message::NowPlaying(n) => {
                o.u8(NOW_PLAYING);
                for s in [&n.package, &n.title, &n.artist, &n.album] {
                    o.str(s);
                }
                o.u8(n.duration_ms.is_some() as u8);
                o.u64(n.duration_ms.unwrap_or(0));
                o.u64(n.position_ms);
                o.u32(n.rate.to_bits());
                o.u8(n.state as u8);
                o.u32(n.commands);
                match &n.artwork {
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
            Message::Gone { package } => {
                o.u8(GONE);
                o.str(package);
            }
            Message::Command { package, command } => {
                o.u8(COMMAND);
                o.str(package);
                let (kind, position) = match command {
                    Command::Play => (0, 0),
                    Command::Pause => (1, 0),
                    Command::TogglePlayPause => (2, 0),
                    Command::Stop => (3, 0),
                    Command::Next => (4, 0),
                    Command::Previous => (5, 0),
                    Command::Seek(ms) => (6, *ms),
                };
                o.u8(kind);
                o.u64(position);
            }
            Message::Consent { id, package, label } => {
                o.u8(CONSENT);
                o.u32(*id);
                o.str(package);
                o.str(label);
            }
            Message::Consented { id, allowed } => {
                o.u8(CONSENTED);
                o.u32(*id);
                o.u8(*allowed as u8);
            }
        }
        o.0
    }

    pub fn decode(bytes: &[u8]) -> io::Result<Message> {
        let mut i = In(bytes);
        let m = match i.u8()? {
            NOW_PLAYING => {
                let (package, title, artist, album) = (i.str()?, i.str()?, i.str()?, i.str()?);
                let known = i.u8()? != 0;
                let duration = i.u64()?;
                let position_ms = i.u64()?;
                let rate = f32::from_bits(i.u32()?);
                let state = match i.u8()? {
                    0 => State::Unknown,
                    1 => State::Playing,
                    2 => State::Paused,
                    3 => State::Stopped,
                    4 => State::Interrupted,
                    _ => return Err(bad()),
                };
                let commands = i.u32()?;
                let artwork = match i.u8()? {
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
                Message::NowPlaying(NowPlaying {
                    package,
                    title,
                    artist,
                    album,
                    duration_ms: known.then_some(duration),
                    position_ms,
                    rate,
                    state,
                    commands,
                    artwork,
                })
            }
            GONE => Message::Gone { package: i.str()? },
            COMMAND => {
                let package = i.str()?;
                let kind = i.u8()?;
                let position = i.u64()?;
                let command = match kind {
                    0 => Command::Play,
                    1 => Command::Pause,
                    2 => Command::TogglePlayPause,
                    3 => Command::Stop,
                    4 => Command::Next,
                    5 => Command::Previous,
                    6 => Command::Seek(position),
                    _ => return Err(bad()),
                };
                Message::Command { package, command }
            }
            CONSENT => Message::Consent {
                id: i.u32()?,
                package: i.str()?,
                label: i.str()?,
            },
            CONSENTED => Message::Consented {
                id: i.u32()?,
                allowed: i.u8()? != 0,
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
            Message::NowPlaying(NowPlaying {
                package: "com.example".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: "Album".into(),
                duration_ms: Some(180_000),
                position_ms: 12_345,
                rate: 1.5,
                state: State::Playing,
                commands: commands::PLAY | commands::SEEK,
                artwork: Some(Image::Rgba {
                    width: 1,
                    height: 1,
                    premultiplied: true,
                    pixels: vec![1, 2, 3, 4],
                }),
            }),
            Message::NowPlaying(NowPlaying {
                package: "p".into(),
                artwork: Some(Image::Encoded(vec![0x89, b'P'])),
                ..Default::default()
            }),
            Message::Gone {
                package: "p".into(),
            },
            Message::Command {
                package: "p".into(),
                command: Command::Seek(42),
            },
            Message::Command {
                package: "p".into(),
                command: Command::TogglePlayPause,
            },
            Message::Consent {
                id: 7,
                package: "p".into(),
                label: "App".into(),
            },
            Message::Consented {
                id: 7,
                allowed: true,
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
    fn refuses_a_short_artwork() {
        let m = Message::NowPlaying(NowPlaying {
            artwork: Some(Image::Rgba {
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
