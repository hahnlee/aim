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
    let Ok(text) = std::fs::read_to_string(publication.directory.join("dynamic_properties"))
    else {
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
        area.get(name).unwrap().map(|read| (read.value.to_vec(), read.serial))
    }

    #[test]
    fn folds_newer_publications_into_the_area() {
        let directory =
            std::env::temp_dir().join(format!("darwin-property-publication-{}", std::process::id()));
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
        assert_eq!(value(&area, b"persist.sys.timezone").unwrap().0, b"Asia/Seoul\0");
        publish(1, "persist.sys.timezone=Asia/Seoul\nsys.boot_completed=1\nro.build.version.sdk=1\n");
        let before = value(&area, b"persist.sys.timezone").unwrap().1;
        fold(&area, &mut publication);
        // Equal values keep their serial, new ones appear, ro.* stays.
        assert_eq!(value(&area, b"persist.sys.timezone").unwrap().1, before);
        assert_eq!(value(&area, b"sys.boot_completed").unwrap().0, b"1\0");
        assert_eq!(value(&area, b"ro.build.version.sdk").unwrap().0, b"36\0");
        publish(2, "persist.sys.timezone=Europe/Paris\nsys.boot_completed=1\n");
        fold(&area, &mut publication);
        let (zone, serial) = value(&area, b"persist.sys.timezone").unwrap();
        assert_eq!(zone, b"Europe/Paris\0");
        assert_ne!(serial, before);
        // The same generation is not read again.
        std::fs::write(directory.join("dynamic_properties"), "persist.sys.timezone=UTC\n").unwrap();
        fold(&area, &mut publication);
        assert_eq!(value(&area, b"persist.sys.timezone").unwrap().0, b"Europe/Paris\0");
        std::fs::remove_dir_all(&directory).unwrap();
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
