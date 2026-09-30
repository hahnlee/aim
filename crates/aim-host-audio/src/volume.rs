//! The Mac's output volume: the default output device's volume as the menu
//! bar shows it (the virtual main volume, over the device's channels) and
//! its mute; read, set and watched, following the user's choice of output
//! device (docs/audio.md, "Volume").

use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use crate::coreaudio::{
    AudioObjectAddPropertyListener, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectRemovePropertyListener, DEFAULT_OUTPUT_DEVICE, OBJECT_SCOPE_GLOBAL,
    OBJECT_SCOPE_OUTPUT, OSStatus, SYSTEM_OBJECT, default_device, fourcc, object_property,
    set_object_property,
};

/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume`.
const VIRTUAL_MAIN_VOLUME: u32 = fourcc(b"vmvc");
/// `kAudioDevicePropertyVolumeScalar`: a channel's own volume, which the
/// virtual main volume is made of.
const VOLUME_SCALAR: u32 = fourcc(b"volm");
/// `kAudioDevicePropertyMute`.
const MUTE: u32 = fourcc(b"mute");
/// `kAudioObjectPropertyElementWildcard`.
const ANY_ELEMENT: u32 = u32::MAX;

/// The default output device's volume, 0 to 1, and its mute.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    pub volume: f32,
    pub muted: bool,
}

/// The default output device's level, or `None` when it has no volume
/// control (a digital output, a display without speakers).
pub fn level() -> Option<Level> {
    let device = default_device(false)?;
    let volume = object_property::<f32>(device, VIRTUAL_MAIN_VOLUME, OBJECT_SCOPE_OUTPUT)?;
    let muted = object_property::<u32>(device, MUTE, OBJECT_SCOPE_OUTPUT).is_some_and(|m| m != 0);
    Some(Level {
        volume: volume.clamp(0.0, 1.0),
        muted,
    })
}

/// Sets the default output device's volume, as the menu bar's slider does.
pub fn set_volume(volume: f32) -> bool {
    default_device(false).is_some_and(|device| {
        set_object_property(
            device,
            VIRTUAL_MAIN_VOLUME,
            OBJECT_SCOPE_OUTPUT,
            &volume.clamp(0.0, 1.0),
        )
    })
}

/// Mutes or unmutes the default output device.
pub fn set_muted(muted: bool) -> bool {
    default_device(false).is_some_and(|device| {
        set_object_property(device, MUTE, OBJECT_SCOPE_OUTPUT, &(muted as u32))
    })
}

struct Watch {
    changed: Box<dyn Fn() + Send + Sync>,
    /// The device whose properties are watched.
    device: Mutex<Option<AudioObjectID>>,
}

static WATCH: OnceLock<Watch> = OnceLock::new();

fn device_addresses() -> [AudioObjectPropertyAddress; 3] {
    [VIRTUAL_MAIN_VOLUME, VOLUME_SCALAR, MUTE].map(|selector| AudioObjectPropertyAddress {
        selector,
        scope: OBJECT_SCOPE_OUTPUT,
        element: ANY_ELEMENT,
    })
}

const DEFAULT_OUTPUT: AudioObjectPropertyAddress = AudioObjectPropertyAddress {
    selector: DEFAULT_OUTPUT_DEVICE,
    scope: OBJECT_SCOPE_GLOBAL,
    element: 0,
};

/// Calls `changed` whenever the level may have changed: the user moved the
/// volume or mute of the default output device (keys, menu bar, another
/// app), or chose another output device. It runs on CoreAudio's
/// notification thread and should only read [`level`]. Once per process;
/// later calls are ignored.
pub fn watch(changed: Box<dyn Fn() + Send + Sync>) {
    let mut first = false;
    let watch = WATCH.get_or_init(|| {
        first = true;
        Watch {
            changed,
            device: Mutex::new(None),
        }
    });
    if !first {
        return;
    }
    // SAFETY: a listener with a static function and no client data.
    unsafe {
        AudioObjectAddPropertyListener(
            SYSTEM_OBJECT,
            &DEFAULT_OUTPUT,
            on_change,
            std::ptr::null_mut(),
        );
    }
    follow_default(watch);
}

/// Moves the device listeners to the current default output device.
fn follow_default(watch: &Watch) {
    let mut watched = watch.device.lock().unwrap();
    let current = default_device(false);
    if *watched == current {
        return;
    }
    for address in device_addresses() {
        // SAFETY: as in `watch`; removing a listener that was added with
        // the same arguments.
        unsafe {
            if let Some(old) = *watched {
                AudioObjectRemovePropertyListener(old, &address, on_change, std::ptr::null_mut());
            }
            if let Some(new) = current {
                AudioObjectAddPropertyListener(new, &address, on_change, std::ptr::null_mut());
            }
        }
    }
    *watched = current;
}

unsafe extern "C" fn on_change(
    object: AudioObjectID,
    _count: u32,
    _addresses: *const AudioObjectPropertyAddress,
    _client_data: *mut c_void,
) -> OSStatus {
    if let Some(watch) = WATCH.get() {
        if object == SYSTEM_OBJECT {
            follow_default(watch);
        }
        (watch.changed)();
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::time::{Duration, Instant};

    fn same(a: Level, b: Level) -> bool {
        (a.volume - b.volume).abs() < 0.01 && a.muted == b.muted
    }

    /// Reads the Mac's level and sets it to what it is: nothing changes.
    /// Skips on a Mac whose output has no volume control.
    #[test]
    fn reads_and_sets_the_level() {
        let Some(before) = level() else {
            eprintln!("the default output has no volume control");
            return;
        };
        assert!(set_volume(before.volume));
        assert!(same(level().unwrap(), before));
    }

    /// Moves the Mac's volume one step (1/16) and back, and hears the
    /// change. Ignored by default: the step is audible over playing sound.
    #[test]
    #[ignore]
    fn watches_a_change_of_the_level() {
        let before = level().expect("the default output has no volume control");
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        watch(Box::new(move || {
            seen.fetch_add(1, SeqCst);
        }));
        let other = if before.volume > 0.5 {
            before.volume - 0.0625
        } else {
            before.volume + 0.0625
        };
        let moved = set_volume(other);
        let after_move = level();
        let deadline = Instant::now() + Duration::from_secs(2);
        while calls.load(SeqCst) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        // Restored before any assertion.
        assert!(set_volume(before.volume));
        assert!(set_muted(before.muted));
        assert!(moved);
        let after_move = after_move.unwrap();
        assert!((after_move.volume - other).abs() < 0.01, "{after_move:?}");
        assert!(calls.load(SeqCst) > 0, "no change notification");
        assert!(same(level().unwrap(), before));
    }
}
