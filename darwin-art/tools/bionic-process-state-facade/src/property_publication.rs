//! The property service's values as every process sees them (ADR 0009, #40).
//!
//! On Android init writes one shared property area that every process maps,
//! so a value the system server sets is visible to all processes once the set
//! returns. Here the profile daemon's property service is init: after it
//! accepts a value it rewrites `properties/dynamic_properties` beside the
//! profile socket (`DARWIN_ART_PROFILE_SOCKET`, the same contract the
//! setter uses) and then bumps the generation in `properties/generation`.
//! Before a property read, a process whose area is older than that generation
//! folds the published values into its area, so readers observe an accepted
//! value after the set returned, as they do on Android.
//!
//! A reader blocked in `__system_property_wait` has no read to trigger that
//! fold, so the service also posts a Darwin notification named after the
//! publication directory's identity. The first wait in a process starts a
//! watcher that folds each new publication into the active area; the area's
//! own update then wakes the waiters, as init's futex wake does on Android.
use crate::properties::PropertyArea;
use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::Mutex;

struct Publication {
    directory: PathBuf,
    generation: Option<File>,
    // The area and publication generation last folded together; a new
    // snapshot's area starts from its launch values again.
    area: usize,
    applied: u64,
}

static PUBLICATION: Mutex<Option<Publication>> = Mutex::new(None);

static WATCHING: Mutex<bool> = Mutex::new(false);

unsafe extern "C" {
    fn notify_register_file_descriptor(
        name: *const std::ffi::c_char,
        descriptor: *mut i32,
        flags: i32,
        token: *mut i32,
    ) -> u32;
}

/// The notification the property service posts after each publication:
/// named by the directory's device and inode, which both sides read the same
/// way whatever path spelling reaches it.
pub(crate) fn notification_name(directory: &std::path::Path) -> Option<std::ffi::CString> {
    use std::os::unix::fs::MetadataExt;
    let metadata = std::fs::metadata(directory).ok()?;
    std::ffi::CString::new(format!(
        "dev.darwinart.properties.{}.{}",
        metadata.dev(),
        metadata.ino()
    ))
    .ok()
}

/// Starts this process's publication watcher once the service has published.
/// Call before the fold that precedes a wait: a publication after the
/// registration wakes the watcher, one before it is in that fold.
pub(crate) fn watch() {
    let Ok(mut watching) = WATCHING.lock() else {
        return;
    };
    if *watching {
        return;
    }
    let Some(name) = directory().as_deref().and_then(notification_name) else {
        return; // Nothing published yet; a later wait retries.
    };
    let mut descriptor = -1;
    let mut token = 0;
    // SAFETY: NUL-terminated name and writable outputs.
    if unsafe { notify_register_file_descriptor(name.as_ptr(), &mut descriptor, 0, &mut token) }
        != 0
    {
        return;
    }
    // SAFETY: notify(3) handed this descriptor to the caller.
    let mut events = unsafe { <File as std::os::fd::FromRawFd>::from_raw_fd(descriptor) };
    let started = std::thread::Builder::new()
        .name("DarwinPropertyWatch".into())
        .spawn(move || {
            use std::io::Read;
            let mut delivered = [0u8; 4];
            while events.read_exact(&mut delivered).is_ok() {
                let _ = crate::property_snapshot();
            }
        });
    *watching = started.is_ok();
}

fn directory() -> Option<PathBuf> {
    let socket = PathBuf::from(std::env::var_os("DARWIN_ART_PROFILE_SOCKET")?);
    Some(socket.parent()?.join("properties"))
}

fn read_generation(file: &File) -> Option<u64> {
    let mut bytes = [0u8; 8];
    file.read_exact_at(&mut bytes, 0).ok()?;
    Some(u64::from_le_bytes(bytes))
}

/// `name=value` lines; a malformed line ends the publication's use.
pub(crate) fn parse(text: &str) -> Option<Vec<(&[u8], &[u8])>> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.split_once('=')
                .map(|(name, value)| (name.as_bytes(), value.as_bytes()))
        })
        .collect()
}

/// Folds a newer publication into `area`. Values already equal are left
/// alone so their serials only move on a real change.
pub(crate) fn refresh(area: &PropertyArea) {
    let Ok(mut guard) = PUBLICATION.lock() else {
        return;
    };
    if guard.is_none() {
        let Some(directory) = directory() else {
            return;
        };
        *guard = Some(Publication {
            directory,
            generation: None,
            area: 0,
            applied: 0,
        });
    }
    fold(area, guard.as_mut().unwrap());
}

fn fold(area: &PropertyArea, publication: &mut Publication) {
    if publication.generation.is_none() {
        // The service publishes once it first opens; until then there is
        // nothing beyond the launch snapshot.
        publication.generation = File::open(publication.directory.join("generation")).ok();
    }
    let Some(generation) = publication.generation.as_ref().and_then(read_generation) else {
        return;
    };
    let area_id = area as *const PropertyArea as usize;
    if area_id == publication.area && generation == publication.applied {
        return;
    }
    let Ok(text) = std::fs::read_to_string(publication.directory.join("dynamic_properties")) else {
        return;
    };
    let Some(values) = parse(&text) else {
        return;
    };
    for (name, value) in values {
        let current = area.get(name).ok().flatten();
        let unchanged = current
            .as_ref()
            .is_some_and(|read| read.value.strip_suffix(&[0]) == Some(value));
        if !unchanged {
            // A rejected value (for example an already-set ro.*) keeps the
            // area's own; the service applied init's policy already.
            let _ = area.update(name, value);
        }
    }
    publication.area = area_id;
    publication.applied = generation;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(area: &PropertyArea, name: &[u8]) -> Option<(Vec<u8>, u32)> {
        area.get(name)
            .unwrap()
            .map(|read| (read.value.to_vec(), read.serial))
    }

    #[test]
    fn folds_newer_publications_into_the_area() {
        let directory = std::env::temp_dir().join(format!(
            "darwin-property-publication-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let publish = |generation: u64, text: &str| {
            std::fs::write(directory.join("dynamic_properties"), text).unwrap();
            std::fs::write(directory.join("generation"), generation.to_le_bytes()).unwrap();
        };
        let area = PropertyArea::new(vec![
            (b"persist.sys.timezone".to_vec(), b"Asia/Seoul".to_vec()),
            (b"ro.build.version.sdk".to_vec(), b"36".to_vec()),
        ])
        .unwrap();
        let mut publication = Publication {
            directory: directory.clone(),
            generation: None,
            area: 0,
            applied: 0,
        };
        // Before the service publishes, the launch values stand.
        fold(&area, &mut publication);
        assert_eq!(
            value(&area, b"persist.sys.timezone").unwrap().0,
            b"Asia/Seoul\0"
        );
        publish(
            1,
            "persist.sys.timezone=Asia/Seoul\nsys.boot_completed=1\nro.build.version.sdk=1\n",
        );
        let before = value(&area, b"persist.sys.timezone").unwrap().1;
        fold(&area, &mut publication);
        // Equal values keep their serial, new ones appear, ro.* stays.
        assert_eq!(value(&area, b"persist.sys.timezone").unwrap().1, before);
        assert_eq!(value(&area, b"sys.boot_completed").unwrap().0, b"1\0");
        assert_eq!(value(&area, b"ro.build.version.sdk").unwrap().0, b"36\0");
        publish(
            2,
            "persist.sys.timezone=Europe/Paris\nsys.boot_completed=1\n",
        );
        fold(&area, &mut publication);
        let (zone, serial) = value(&area, b"persist.sys.timezone").unwrap();
        assert_eq!(zone, b"Europe/Paris\0");
        assert_ne!(serial, before);
        // The same generation is not read again.
        std::fs::write(
            directory.join("dynamic_properties"),
            "persist.sys.timezone=UTC\n",
        )
        .unwrap();
        fold(&area, &mut publication);
        assert_eq!(
            value(&area, b"persist.sys.timezone").unwrap().0,
            b"Europe/Paris\0"
        );
        std::fs::remove_dir_all(&directory).unwrap();
    }

    unsafe extern "C" {
        fn notify_post(name: *const std::ffi::c_char) -> u32;
    }

    #[test]
    fn remote_publication_wakes_a_blocked_wait() {
        // The watcher and DARWIN_ART_PROFILE_SOCKET are process-wide, so run
        // in a copy of this binary holding only this test.
        const PROFILE: &str = "DARWIN_ART_PROPERTY_WATCH_PROFILE";
        let Some(profile) = std::env::var_os(PROFILE) else {
            let profile =
                std::env::temp_dir().join(format!("darwin-property-watch-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&profile);
            std::fs::create_dir_all(profile.join("properties")).unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "property_publication::tests::remote_publication_wakes_a_blocked_wait",
                    "--exact",
                    "--test-threads=1",
                ])
                .env(PROFILE, &profile)
                .env("DARWIN_ART_PROFILE_SOCKET", profile.join("profile.sock"))
                .status()
                .unwrap();
            std::fs::remove_dir_all(&profile).unwrap();
            assert!(status.success());
            return;
        };
        let directory = PathBuf::from(profile).join("properties");
        let publish = |generation: u64, text: &str| {
            std::fs::write(directory.join("dynamic_properties"), text).unwrap();
            std::fs::write(directory.join("generation"), generation.to_le_bytes()).unwrap();
        };
        publish(1, "sys.boot_completed=0\n");
        let snapshot = std::sync::Arc::new(
            crate::Snapshot::new(
                vec![],
                vec![(b"sys.boot_completed".to_vec(), b"0".to_vec())],
                crate::AuxSnapshot {
                    page_size: 16384,
                    hwcap: 3,
                    hwcap2: 0,
                    secure: false,
                    random: [0; 16],
                },
            )
            .unwrap(),
        );
        let _active = snapshot.activate().unwrap();
        let token = snapshot.properties.find(b"sys.boot_completed").unwrap() as usize;
        let before = snapshot
            .properties
            .serial(token as *const _)
            .unwrap()
            .unwrap();
        let waiter = std::thread::spawn(move || {
            let mut output = 0;
            let timeout = crate::property_wait_abi::WaitTimeout {
                seconds: 10,
                nanoseconds: 0,
            };
            let woke = unsafe {
                crate::property_wait_abi::darwin_art_bionic_process_property_wait_core(
                    token as *const _,
                    before,
                    &mut output,
                    &timeout,
                )
            };
            (woke, output)
        });
        // The property service in another process publishes, then posts.
        std::thread::sleep(std::time::Duration::from_millis(300));
        publish(2, "sys.boot_completed=1\n");
        let name = notification_name(&directory).unwrap();
        assert_eq!(unsafe { notify_post(name.as_ptr()) }, 0);
        let (woke, output) = waiter.join().unwrap();
        assert_eq!(woke, 1, "the wait timed out instead of waking");
        assert_ne!(output, before);
        assert_eq!(
            value(&snapshot.properties, b"sys.boot_completed")
                .unwrap()
                .0,
            b"1\0"
        );
    }

    #[test]
    fn parses_published_lines() {
        assert_eq!(
            parse("persist.sys.timezone=Asia/Seoul\nsys.boot_completed=1\n"),
            Some(vec![
                (&b"persist.sys.timezone"[..], &b"Asia/Seoul"[..]),
                (&b"sys.boot_completed"[..], &b"1"[..]),
            ])
        );
        assert_eq!(parse("a=b=c"), Some(vec![(&b"a"[..], &b"b=c"[..])]));
        assert_eq!(parse("broken"), None);
    }
}
