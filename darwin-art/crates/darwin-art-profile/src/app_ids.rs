//! Package-manager app IDs, independent of Darwin uid and process IDs.
//! Access is serialized by PackageRegistry's daemon mutex.
use crate::{ProfileError, registry::validate_package};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

const FIRST: u32 = 10_000;
const LAST: u32 = 19_999;
const VERSION: &str = "darwin-art-app-ids-v1";

fn parse(text: &str) -> Result<BTreeMap<String, u32>, ProfileError> {
    let invalid = || ProfileError::Daemon("invalid package app-ID database".into());
    let mut lines = text.lines();
    if lines.next() != Some(VERSION) || !text.ends_with('\n') {
        return Err(invalid());
    }
    let mut ids = BTreeMap::new();
    let mut used = BTreeSet::new();
    for line in lines {
        let (package, id) = line.split_once('\t').ok_or_else(invalid)?;
        validate_package(package)?;
        let id: u32 = id.parse().map_err(|_| invalid())?;
        if !(FIRST..=LAST).contains(&id)
            || ids.insert(package.into(), id).is_some()
            || !used.insert(id)
        {
            return Err(invalid());
        }
    }
    Ok(ids)
}

pub(crate) fn allocate(directory: &Path, package: &str) -> Result<u32, ProfileError> {
    validate_package(package)?;
    let path = directory.join("app-ids");
    let mut ids = match fs::read_to_string(&path) {
        Ok(text) => parse(&text)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // A lost ledger is not a fresh installation. Reallocating could
            // silently change ownership of existing package data.
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                if entry.path().extension().is_some_and(|ext| ext == "launch") {
                    let record = fs::read(entry.path())?;
                    if record
                        .split(|byte| *byte == b'\n')
                        .any(|line| line.starts_with(b"app_id="))
                    {
                        return Err(ProfileError::Daemon(
                            "app-ID database missing for assigned packages".into(),
                        ));
                    }
                }
            }
            BTreeMap::new()
        }
        Err(error) => return Err(error.into()),
    };
    if let Some(id) = ids.get(package) {
        return Ok(*id);
    }
    let used: BTreeSet<u32> = ids.values().copied().collect();
    let id = (FIRST..=LAST)
        .find(|id| !used.contains(id))
        .ok_or_else(|| ProfileError::Daemon("Android app-ID range exhausted".into()))?;
    ids.insert(package.into(), id);
    let mut text = format!("{VERSION}\n");
    for (package, id) in ids {
        text.push_str(&format!("{package}\t{id}\n"));
    }
    // Preserve allocations across upgrade/uninstall until data and process
    // teardown support safe reclamation. No hash-based IDs or host uid reuse.
    let stage = directory.join(format!(".app-ids-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&stage)?;
    let result = file
        .write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&stage, &path))
        .and_then(|()| File::open(directory)?.sync_all());
    if let Err(error) = result {
        let _ = fs::remove_file(&stage);
        return Err(error.into());
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_duplicates_and_privileged_ids() {
        for rows in ["a\t1000\n", "a\t10000\nb\t10000\n", "a\t10000\na\t10001\n"] {
            assert!(parse(&format!("{VERSION}\n{rows}")).is_err());
        }
        assert!(parse("unknown\n").is_err());
    }
    #[test]
    fn allocations_survive_reload_and_remain_distinct() {
        let dir = std::env::temp_dir().join(format!(
            "darwin-app-ids-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        assert_eq!(allocate(&dir, "org.example.first").unwrap(), FIRST);
        assert_eq!(allocate(&dir, "org.example.second").unwrap(), FIRST + 1);
        assert_eq!(allocate(&dir, "org.example.first").unwrap(), FIRST);
        fs::write(dir.join("app-ids"), "corrupt\n").unwrap();
        assert!(allocate(&dir, "org.example.third").is_err());
        fs::remove_file(dir.join("app-ids")).unwrap();
        fs::remove_dir(&dir).unwrap();
    }
}
