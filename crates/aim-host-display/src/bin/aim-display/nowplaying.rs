//! The Mac's Now Playing (`docs/media.md`), in the process that stands
//! for the session's app (its shim; the server in device mode): the
//! session as `MPNowPlayingInfoCenter`'s info and playback state, and
//! `MPRemoteCommandCenter`'s commands, which the Mac's media keys, the
//! menu bar's Now Playing and Control Center send, back to the session.

use std::ffi::c_void;
use std::sync::Mutex;

use aim_host_display::media::{Command, Message, NowPlaying, State, commands};
use aim_host_display::notify::Image;

use crate::objc::{CGSize, GlobalBlock, Id, class, nsstring, on_main, release, retain};

#[link(name = "MediaPlayer", kind = "framework")]
unsafe extern "C" {
    static MPMediaItemPropertyTitle: Id;
    static MPMediaItemPropertyArtist: Id;
    static MPMediaItemPropertyAlbumTitle: Id;
    static MPMediaItemPropertyPlaybackDuration: Id;
    static MPMediaItemPropertyArtwork: Id;
    static MPNowPlayingInfoPropertyElapsedPlaybackTime: Id;
    static MPNowPlayingInfoPropertyPlaybackRate: Id;
}

/// `MPRemoteCommandHandlerStatus`: success, command failed.
const SUCCESS: isize = 0;
const FAILED: isize = 200;
/// `NSBitmapFormatAlphaNonpremultiplied`.
const NON_PREMULTIPLIED: usize = 1 << 1;

/// The package whose session this process shows.
static SHOWN: Mutex<Option<String>> = Mutex::new(None);
/// The artwork's image (a retained `NSImage`, as an address), which the
/// artwork's request handler returns.
static ARTWORK: Mutex<usize> = Mutex::new(0);

/// The remote commands, the command each sends and the flag of
/// [`commands`] that enables it.
const COMMANDS: [(&std::ffi::CStr, u8, u32); 7] = [
    (c"playCommand", 0, commands::PLAY),
    (c"pauseCommand", 1, commands::PAUSE),
    (
        c"togglePlayPauseCommand",
        2,
        commands::PLAY | commands::PAUSE,
    ),
    (c"stopCommand", 3, commands::STOP),
    (c"nextTrackCommand", 4, commands::NEXT),
    (c"previousTrackCommand", 5, commands::PREVIOUS),
    (c"changePlaybackPositionCommand", 6, commands::SEEK),
];

fn info_center() -> Id {
    send!(class(c"MPNowPlayingInfoCenter"), c"defaultCenter" => Id)
}

fn command_center() -> Id {
    send!(class(c"MPRemoteCommandCenter"), c"sharedCommandCenter" => Id)
}

/// A remote command's handler: its command for the session shown.
extern "C" fn on_command<const K: u8>(_block: *const GlobalBlock, event: Id) -> isize {
    let Some(package) = SHOWN.lock().unwrap().clone() else {
        return FAILED;
    };
    let command = match K {
        0 => Command::Play,
        1 => Command::Pause,
        2 => Command::TogglePlayPause,
        3 => Command::Stop,
        4 => Command::Next,
        5 => Command::Previous,
        _ => {
            let seconds = send!(event, c"positionTime" => f64);
            Command::Seek((seconds.max(0.0) * 1000.0) as u64)
        }
    };
    crate::media::answer(&Message::Command { package, command });
    SUCCESS
}

/// The artwork's request handler: the current image, whatever size is
/// asked (the Mac scales it).
extern "C" fn artwork(_block: *const GlobalBlock, _size: CGSize) -> Id {
    let image = *ARTWORK.lock().unwrap() as Id;
    // Alive for the caller, even once replaced.
    send!(retain(image), c"autorelease" => Id)
}

/// Adds the command handlers, once.
fn register_commands() {
    static BLOCKS: std::sync::OnceLock<Vec<GlobalBlock>> = std::sync::OnceLock::new();
    if BLOCKS.get().is_some() {
        return;
    }
    let invoke = [
        on_command::<0> as *const c_void,
        on_command::<1> as *const c_void,
        on_command::<2> as *const c_void,
        on_command::<3> as *const c_void,
        on_command::<4> as *const c_void,
        on_command::<5> as *const c_void,
        on_command::<6> as *const c_void,
    ];
    let blocks = BLOCKS.get_or_init(|| invoke.iter().map(|&f| GlobalBlock::new(f)).collect());
    let center = command_center();
    for ((name, _, _), block) in COMMANDS.iter().zip(blocks) {
        let command = send!(center, name => Id);
        send!(command, c"addTargetWithHandler:" => Id, *const GlobalBlock = block);
    }
}

/// Enables the commands `mask` allows (none: every command off).
fn enable_commands(mask: u32) {
    let center = command_center();
    for (name, _, flags) in COMMANDS {
        let command = send!(center, name => Id);
        send!(command, c"setEnabled:" => (), bool = mask & flags != 0);
    }
}

fn number(v: f64) -> Id {
    send!(class(c"NSNumber"), c"numberWithDouble:" => Id, f64 = v)
}

/// `image` as an `NSImage` (retained), if it can be decoded.
fn ns_image(image: &Image) -> Id {
    match image {
        Image::Rgba {
            width,
            height,
            premultiplied,
            pixels,
        } => {
            let (w, h) = (*width as isize, *height as isize);
            let rep = send!(class(c"NSBitmapImageRep"), c"alloc" => Id);
            // Planes of null: the rep allocates its own pixels.
            let rep = send!(rep,
                c"initWithBitmapDataPlanes:pixelsWide:pixelsHigh:bitsPerSample:samplesPerPixel:hasAlpha:isPlanar:colorSpaceName:bitmapFormat:bytesPerRow:bitsPerPixel:" => Id,
                *mut *mut u8 = std::ptr::null_mut(), isize = w, isize = h, isize = 8, isize = 4,
                bool = true, bool = false, Id = nsstring("NSDeviceRGBColorSpace"),
                usize = if *premultiplied { 0 } else { NON_PREMULTIPLIED },
                isize = w * 4, isize = 32);
            if rep.is_null() {
                return rep;
            }
            let data = send!(rep, c"bitmapData" => *mut u8);
            // SAFETY: the rep's own buffer of w * h * 4 bytes, as asked.
            unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), data, pixels.len()) };
            let size = CGSize {
                width: w as f64,
                height: h as f64,
            };
            let ns = send!(class(c"NSImage"), c"alloc" => Id);
            let ns = send!(ns, c"initWithSize:" => Id, CGSize = size);
            send!(ns, c"addRepresentation:" => (), Id = rep);
            release(rep);
            ns
        }
        Image::Encoded(bytes) => {
            let data = send!(class(c"NSData"), c"dataWithBytes:length:" => Id,
                *const u8 = bytes.as_ptr(), usize = bytes.len());
            let ns = send!(class(c"NSImage"), c"alloc" => Id);
            send!(ns, c"initWithData:" => Id, Id = data)
        }
    }
}

/// Show `n` as the Mac's Now Playing.
pub fn show(n: NowPlaying) {
    on_main(move || {
        let _pool = crate::objc::Pool::new();
        register_commands();
        enable_commands(n.commands);
        let info = send!(class(c"NSMutableDictionary"), c"dictionary" => Id);
        let set = |key: Id, value: Id| {
            if !value.is_null() {
                send!(info, c"setObject:forKey:" => (), Id = value, Id = key);
            }
        };
        // SAFETY: MediaPlayer's constant keys.
        unsafe {
            set(MPMediaItemPropertyTitle, nsstring(&n.title));
            if !n.artist.is_empty() {
                set(MPMediaItemPropertyArtist, nsstring(&n.artist));
            }
            if !n.album.is_empty() {
                set(MPMediaItemPropertyAlbumTitle, nsstring(&n.album));
            }
            if let Some(d) = n.duration_ms {
                set(
                    MPMediaItemPropertyPlaybackDuration,
                    number(d as f64 / 1000.0),
                );
            }
            set(
                MPNowPlayingInfoPropertyElapsedPlaybackTime,
                number(n.position_ms as f64 / 1000.0),
            );
            set(
                MPNowPlayingInfoPropertyPlaybackRate,
                number(f64::from(n.rate)),
            );
        }
        let image = n.artwork.as_ref().map_or(std::ptr::null_mut(), ns_image);
        let old = std::mem::replace(&mut *ARTWORK.lock().unwrap(), image as usize);
        release(old as Id);
        if !image.is_null() {
            static HANDLER: std::sync::OnceLock<GlobalBlock> = std::sync::OnceLock::new();
            let handler = HANDLER.get_or_init(|| GlobalBlock::new(artwork as *const c_void));
            let size = send!(image, c"size" => CGSize);
            let item = send!(class(c"MPMediaItemArtwork"), c"alloc" => Id);
            let item = send!(item, c"initWithBoundsSize:requestHandler:" => Id,
                CGSize = size, *const GlobalBlock = handler);
            // SAFETY: MediaPlayer's constant key.
            set(unsafe { MPMediaItemPropertyArtwork }, item);
            release(item);
        }
        let center = info_center();
        send!(center, c"setNowPlayingInfo:" => (), Id = info);
        send!(center, c"setPlaybackState:" => (), usize = n.state as usize);
        *SHOWN.lock().unwrap() = Some(n.package);
    });
}

/// `package` no longer has the session: Now Playing no longer shows this
/// process.
pub fn clear(package: String) {
    on_main(move || {
        let mut shown = SHOWN.lock().unwrap();
        if shown.as_deref() != Some(package.as_str()) {
            return;
        }
        *shown = None;
        enable_commands(0);
        let center = info_center();
        send!(center, c"setNowPlayingInfo:" => (), Id = std::ptr::null_mut());
        send!(center, c"setPlaybackState:" => (), usize = State::Stopped as usize);
        release(std::mem::take(&mut *ARTWORK.lock().unwrap()) as Id);
    });
}
