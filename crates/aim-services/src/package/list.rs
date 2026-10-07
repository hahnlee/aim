//! `/data/system/packages.list`: the uid list native daemons read
//! (installd, run-as, the sdcard and SELinux setup) through
//! libpackagelistparser, one line per package as
//! `Settings.writePackageListLPrInternal` writes it.
//! Ported from android-16.0.0_r1, Copyright (C) The Android Open Source
//! Project, Apache License 2.0.

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub uid: u32,
    pub debuggable: bool,
    /// User 0's data directory, or `null`.
    pub data_dir: String,
    /// The SELinux `seinfo` label, with its `:`-separated attributes.
    pub seinfo: String,
    /// Supplementary gids, from the package's permissions.
    pub gids: Vec<u32>,
    pub profileable_from_shell: bool,
    pub version_code: i64,
    pub profileable: bool,
    /// The installer's package, or `@system`, `@product` or `@null`.
    pub installer: String,
}

/// Rows from the exact native scan/query generation. Permission GIDs are
/// resolved separately from the active-user owner before committing the file.
pub(crate) fn metadata_from_capture(
    capture: &super::scan_snapshot::query_state::Capture,
) -> Result<Vec<Entry>, String> {
    use super::pkg::{booleans, booleans2};
    let state = capture.state();
    let mut settings: Vec<_> = capture.scan().owner().settings.packages.iter().collect();
    settings.sort_by_key(|p| super::info::java_hash(&p.name));
    let mut entries = Vec::new();
    for setting in settings {
        let ps = state
            .packages
            .get(&setting.name)
            .ok_or("missing captured package setting")?;
        let Some(pkg) = &ps.pkg else { continue };
        if pkg.is2(booleans2::APEX) {
            continue;
        }
        let user = ps
            .users
            .get(&0)
            .ok_or("packages.list requires captured system-user state")?;
        let nullable = state
            .system
            .flags
            .iter()
            .any(|(name, enabled)| name == super::info::NULLABLE_DATA_DIR && *enabled);
        let data_dir = if ps.name == "android" {
            "/data/system".into()
        } else if !user.installed && !user.data_exists && nullable {
            "null".into()
        } else {
            let base = ps
                .volume_uuid
                .as_ref()
                .map(|v| format!("/mnt/expand/{v}"))
                .unwrap_or_else(|| "/data".into());
            let kind = if ps.is.default_to_device_protected_storage {
                "user_de"
            } else {
                "user"
            };
            format!("{base}/{kind}/0/{}", ps.name)
        };
        if data_dir.contains(' ') {
            continue;
        }
        let profileable = !pkg.is(booleans::DISALLOW_PROFILING);
        entries.push(Entry {
            name: pkg.package_name.clone(),
            uid: pkg
                .uid
                .try_into()
                .map_err(|_| "invalid loaded package UID")?,
            debuggable: pkg.is(booleans::DEBUGGABLE),
            data_dir,
            seinfo: ps.seinfo.clone().unwrap_or_else(|| "null".into()),
            gids: Vec::new(),
            profileable_from_shell: profileable && pkg.is(booleans::PROFILEABLE_BY_SHELL),
            version_code: (i64::from(pkg.version_code_major) << 32)
                | i64::from(pkg.version_code as u32),
            profileable,
            installer: if ps.is.system {
                "@system".into()
            } else if ps.is.product {
                "@product".into()
            } else {
                ps.install_source
                    .installer
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| "@null".into())
            },
        });
    }
    serialize(&entries)?;
    Ok(entries)
}

/// Settings' ten-field wire format. Preserve GID order and duplicates from the
/// permission owner; tokens cannot contain whitespace or inject another row.
pub fn serialize(entries: &[Entry]) -> Result<String, String> {
    use std::fmt::Write;
    let mut output = String::new();
    let mut names = std::collections::BTreeSet::new();
    for entry in entries {
        for token in [
            &entry.name,
            &entry.data_dir,
            &entry.seinfo,
            &entry.installer,
        ] {
            if token.is_empty() || token.bytes().any(|b| b.is_ascii_whitespace()) {
                return Err("packages.list requires nonempty single-token fields".into());
            }
        }
        if !names.insert(&entry.name) {
            return Err("duplicate packages.list package".into());
        }
        let gids = if entry.gids.is_empty() {
            "none".into()
        } else {
            entry
                .gids
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        writeln!(
            &mut output,
            "{} {} {} {} {} {} {} {} {} {}",
            entry.name,
            entry.uid,
            u8::from(entry.debuggable),
            entry.data_dir,
            entry.seinfo,
            gids,
            u8::from(entry.profileable_from_shell),
            entry.version_code,
            u8::from(entry.profileable),
            entry.installer
        )
        .unwrap();
    }
    Ok(output)
}

/// The entries of `text`. The last four fields are optional, as for
/// libpackagelistparser (which reads up to the version code); a line
/// without one of the first six is an error.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    text.lines()
        .enumerate()
        .map(|(i, line)| entry(line).ok_or_else(|| format!("packages.list:{}: {line:?}", i + 1)))
        .collect()
}

fn entry(line: &str) -> Option<Entry> {
    let mut f = line.split_ascii_whitespace();
    let flag = |s: Option<&str>| s.map(|s| s.parse::<i32>().map(|v| v != 0)).transpose();
    let mut e = Entry {
        name: f.next()?.to_owned(),
        uid: f.next()?.parse().ok()?,
        debuggable: flag(f.next()).ok()??,
        data_dir: f.next()?.to_owned(),
        seinfo: f.next()?.to_owned(),
        gids: match f.next()? {
            "none" => Vec::new(),
            gids => gids
                .split(',')
                .map(|g| g.parse().ok())
                .collect::<Option<_>>()?,
        },
        profileable_from_shell: false,
        version_code: 0,
        profileable: false,
        installer: String::new(),
    };
    e.profileable_from_shell = flag(f.next()).ok()?.unwrap_or(false);
    e.version_code = f.next().map(str::parse).transpose().ok()?.unwrap_or(0);
    e.profileable = flag(f.next()).ok()?.unwrap_or(false);
    e.installer = f.next().unwrap_or_default().to_owned();
    Some(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_package_manager_writes() {
        let text = "\
com.android.networkstack 1073 0 /data/user_de/0/com.android.networkstack network_stack:privapp:targetSdkVersion=36:partition=system 3002,3003 0 36 1 @system
org.example.app 10213 1 /data/user/0/org.example.app default:targetSdkVersion=35 none 1 7 0 com.android.shell
org.example.old 10214 0 /data/user/0/org.example.old default none
";
        let e = parse(text).unwrap();
        assert_eq!(e.len(), 3);
        assert_eq!((e[0].uid, &e[0].gids[..]), (1073, &[3002, 3003][..]));
        assert_eq!(e[0].installer, "@system");
        assert!(e[0].profileable && !e[0].debuggable);
        assert_eq!(e[1].name, "org.example.app");
        assert!(e[1].debuggable && e[1].profileable_from_shell && !e[1].profileable);
        assert_eq!(
            (e[1].version_code, e[1].installer.as_str()),
            (7, "com.android.shell")
        );
        assert!(e[2].gids.is_empty());
        assert_eq!(e[2].version_code, 0);
        assert!(parse("org.example 10000 0 /data/x default\n").is_err());
        assert!(parse("org.example 10000 0 /data/x default 1,x\n").is_err());
    }
}
