//! The media parcels the bridge reads and writes, as their Java classes
//! write them at the pinned tag: `MediaSession.Token`, `PlaybackState`,
//! `MediaMetadata` (a `Bundle` of text, longs and bitmaps) and
//! `ActivityOptions.LaunchCookie`.

use aim_binder_host::parcel::{Binder, Parcel, Reader, Result};
use aim_host_display::notify::Image;

use crate::notifications::parcels::{Files, Icon, parcelable_image, text_value, walk_bundle};

/// `Parcel.VAL_PARCELABLE`, `VAL_LONG`.
const VAL_PARCELABLE: i32 = 4;
const VAL_LONG: i32 = 6;

/// `PlaybackState.STATE_*`.
pub const STATE_NONE: i32 = 0;
pub const STATE_STOPPED: i32 = 1;
pub const STATE_PAUSED: i32 = 2;
pub const STATE_PLAYING: i32 = 3;
pub const STATE_FAST_FORWARDING: i32 = 4;
pub const STATE_REWINDING: i32 = 5;
pub const STATE_ERROR: i32 = 7;

/// `PlaybackState.ACTION_*`.
pub const ACTION_STOP: i64 = 1 << 0;
pub const ACTION_PAUSE: i64 = 1 << 1;
pub const ACTION_PLAY: i64 = 1 << 2;
pub const ACTION_SKIP_TO_PREVIOUS: i64 = 1 << 4;
pub const ACTION_SKIP_TO_NEXT: i64 = 1 << 5;
pub const ACTION_SEEK_TO: i64 = 1 << 8;
pub const ACTION_PLAY_PAUSE: i64 = 1 << 9;

/// `MediaSession.Token`: the session's uid and its `ISessionController`.
#[derive(Debug)]
pub struct Token {
    pub controller: Option<Binder>,
}

impl aim_service_aidl::ReadParcelable for Token {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        r.read_i32()?; // uid
        Ok(Token {
            controller: r.read_binder()?,
        })
    }
}

/// `PlaybackState`, up to its actions (custom actions, the active item,
/// the error and the extras follow).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PlaybackState {
    pub state: i32,
    pub position: i64,
    pub speed: f32,
    /// `SystemClock.elapsedRealtime()` when `position` was current.
    pub update_time: i64,
    pub actions: i64,
}

impl aim_service_aidl::ReadParcelable for PlaybackState {
    fn read_from(r: &mut Reader<'_>) -> Result<Self> {
        let state = r.read_i32()?;
        let position = r.read_i64()?;
        let speed = r.read_f32()?;
        let update_time = r.read_i64()?;
        r.read_i64()?; // buffered position
        let actions = r.read_i64()?;
        Ok(PlaybackState {
            state,
            position,
            speed,
            update_time,
            actions,
        })
    }
}

impl PlaybackState {
    /// Whether the position advances: `PlaybackState.isActive` less the
    /// states that wait (buffering, connecting, skipping).
    pub fn advancing(&self) -> bool {
        matches!(
            self.state,
            STATE_PLAYING | STATE_FAST_FORWARDING | STATE_REWINDING
        )
    }

    /// The position at `now` (of `elapsedRealtime`), as a controller
    /// extrapolates it, within `duration` if known.
    pub fn position_at(&self, now: i64, duration: Option<i64>) -> i64 {
        let mut p = self.position;
        if self.advancing() && self.update_time > 0 && now > self.update_time {
            p += ((now - self.update_time) as f64 * f64::from(self.speed)) as i64;
        }
        let p = p.max(0);
        duration.filter(|&d| d > 0).map_or(p, |d| p.min(d))
    }
}

/// What `MediaMetadata` holds that Now Playing shows, with SystemUI's
/// media controls' preferences: the display title before the title, the
/// artwork before the album art before the display icon.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metadata {
    pub title: Option<String>,
    pub display_title: Option<String>,
    pub artist: Option<String>,
    pub album_artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<i64>,
    pub art: Option<Image>,
    pub album_art: Option<Image>,
    pub display_icon: Option<Image>,
}

impl Metadata {
    /// `MediaMetadata(Parcel)`: its bundle, then the bitmap size limit.
    pub fn read(r: &mut Reader<'_>, files: &dyn Files) -> Result<Metadata> {
        let mut m = Metadata::default();
        walk_bundle(r, &mut |key, kind, r| {
            let slot = match key {
                "android.media.metadata.TITLE" => &mut m.title,
                "android.media.metadata.DISPLAY_TITLE" => &mut m.display_title,
                "android.media.metadata.ARTIST" => &mut m.artist,
                "android.media.metadata.ALBUM_ARTIST" => &mut m.album_artist,
                "android.media.metadata.ALBUM" => &mut m.album,
                "android.media.metadata.DURATION" if kind == VAL_LONG => {
                    m.duration = Some(r.read_i64()?);
                    return Ok(true);
                }
                "android.media.metadata.ART"
                | "android.media.metadata.ALBUM_ART"
                | "android.media.metadata.DISPLAY_ICON"
                    if kind == VAL_PARCELABLE =>
                {
                    let image = match parcelable_image(r, files)? {
                        Some(Icon::Image(i)) => Some(i),
                        _ => None,
                    };
                    match key {
                        "android.media.metadata.ART" => m.art = image,
                        "android.media.metadata.ALBUM_ART" => m.album_art = image,
                        _ => m.display_icon = image,
                    }
                    return Ok(true);
                }
                _ => return Ok(false),
            };
            *slot = text_value(r, kind)?.filter(|s| !s.is_empty());
            Ok(slot.is_some())
        })?;
        r.read_i32()?; // bitmap dimension limit
        Ok(m)
    }

    pub fn title(&self) -> Option<&str> {
        self.display_title.as_deref().or(self.title.as_deref())
    }

    pub fn artist(&self) -> Option<&str> {
        self.artist.as_deref().or(self.album_artist.as_deref())
    }

    pub fn artwork(&self) -> Option<&Image> {
        [&self.art, &self.album_art, &self.display_icon]
            .into_iter()
            .flatten()
            .next()
    }
}

/// `ActivityOptions.LaunchCookie`: its binder.
pub struct LaunchCookie(pub Binder);

impl aim_service_aidl::WriteParcelable for LaunchCookie {
    fn write_to(&self, p: &mut Parcel) {
        p.write_binder(Some(self.0));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoFiles;

    impl Files for NoFiles {
        fn read(&self, _fd: u32, _len: usize) -> Option<Vec<u8>> {
            None
        }
    }

    fn string16(p: &mut Parcel, s: &str) {
        p.write_string16(Some(s));
    }

    /// `MediaMetadata.writeToParcel` of a bundle as `MediaMetadata.Builder`
    /// fills it: text as `CharSequence`, the duration as a long, the art
    /// as a length-prefixed `Bitmap`.
    fn java_metadata() -> Vec<u8> {
        let mut entries = Parcel::new();
        entries.write_i32(4);
        string16(&mut entries, "android.media.metadata.TITLE");
        entries.write_i32(10); // VAL_CHARSEQUENCE: TextUtils, a plain string
        entries.write_i32(1);
        entries.write_string8(Some("Song"));
        string16(&mut entries, "android.media.metadata.ARTIST");
        entries.write_i32(0); // VAL_STRING
        string16(&mut entries, "Artist");
        string16(&mut entries, "android.media.metadata.DURATION");
        entries.write_i32(VAL_LONG);
        entries.write_i64(180_000);
        string16(&mut entries, "android.media.metadata.ART");
        entries.write_i32(VAL_PARCELABLE);
        let mut bitmap = Parcel::new();
        string16(&mut bitmap, "android.graphics.Bitmap");
        bitmap.write_i32(0); // immutable
        bitmap.write_i32(4); // RGBA_8888
        bitmap.write_i32(2); // premultiplied
        bitmap.write_i32(-1); // no color space
        bitmap.write_i32(1);
        bitmap.write_i32(1);
        bitmap.write_i32(4); // row bytes
        bitmap.write_i32(160);
        bitmap.write_i64(0);
        bitmap.write_i32(0); // in place
        bitmap.write_i32(4);
        bitmap.write_i32(i32::from_le_bytes([1, 2, 3, 255]));
        entries.write_i32(bitmap.data().len() as i32);
        entries.write_raw(bitmap.data(), &[]);
        let mut p = Parcel::new();
        p.write_i32(entries.data().len() as i32);
        p.write_i32(0x4C44_4E42); // BUNDLE_MAGIC
        p.write_raw(entries.data(), &[]);
        p.write_bool(false); // no intent
        p.write_i32(320); // bitmap dimension limit
        p.data().to_vec()
    }

    #[test]
    fn reads_metadata() {
        let bytes = java_metadata();
        let mut r = Reader::new(&bytes, &[]);
        let m = Metadata::read(&mut r, &NoFiles).unwrap();
        assert_eq!(m.title(), Some("Song"));
        assert_eq!(m.artist(), Some("Artist"));
        assert_eq!(m.duration, Some(180_000));
        assert_eq!(
            m.artwork(),
            Some(&Image::Rgba {
                width: 1,
                height: 1,
                premultiplied: true,
                pixels: vec![1, 2, 3, 255],
            })
        );
    }

    #[test]
    fn extrapolates_the_position() {
        let s = PlaybackState {
            state: STATE_PLAYING,
            position: 1_000,
            speed: 2.0,
            update_time: 10_000,
            actions: 0,
        };
        assert_eq!(s.position_at(10_500, None), 2_000);
        assert_eq!(s.position_at(10_500, Some(1_500)), 1_500);
        let paused = PlaybackState {
            state: STATE_PAUSED,
            ..s
        };
        assert_eq!(paused.position_at(10_500, None), 1_000);
    }
}
