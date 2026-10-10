//! RuntimePermissionsPersistenceImpl.serializeRuntimePermissions, Android 16 r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::RuntimePermissions;
use std::fmt::Write;

fn attribute(output: &mut String, name: &str, value: &str) -> Result<(), String> {
    write!(output, " {name}=\"").unwrap();
    for c in value.chars() {
        match c {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\t' => output.push_str("&#9;"),
            '\n' => output.push_str("&#10;"),
            '\r' => output.push_str("&#13;"),
            c if c < '\u{20}' || c == '\u{fffe}' || c == '\u{ffff}' => {
                return Err("invalid XML attribute character".into());
            }
            c => output.push(c),
        }
    }
    output.push('"');
    Ok(())
}

impl RuntimePermissions {
    /// Text XML, retaining owner order and empty package/shared states. Version
    /// and fingerprint must come from the runtime permission persistence owner.
    pub fn serialize(&self) -> Result<Vec<u8>, String> {
        let mut output = String::from(
            "<?xml version='1.0' encoding='UTF-8' standalone='yes' ?><runtime-permissions",
        );
        attribute(&mut output, "version", &self.version.to_string())?;
        if let Some(fingerprint) = &self.fingerprint {
            attribute(&mut output, "fingerprint", fingerprint)?;
        }
        output.push('>');
        for (tag, owners) in [
            ("package", &self.packages),
            ("shared-user", &self.shared_users),
        ] {
            for (name, permissions) in owners {
                write!(output, "<{tag}").unwrap();
                attribute(
                    &mut output,
                    "name",
                    name.as_deref()
                        .ok_or("null runtime owner cannot be serialized")?,
                )?;
                output.push('>');
                for permission in permissions {
                    output.push_str("<permission");
                    attribute(
                        &mut output,
                        "name",
                        permission
                            .name
                            .as_deref()
                            .ok_or("null runtime permission cannot be serialized")?,
                    )?;
                    // A one-time grant must not survive a permission-state restart.
                    let granted = permission.granted && permission.flags & (1 << 16) == 0;
                    attribute(
                        &mut output,
                        "granted",
                        if granted { "true" } else { "false" },
                    )?;
                    attribute(
                        &mut output,
                        "flags",
                        &format!("{:x}", permission.flags as u32),
                    )?;
                    output.push_str(" />");
                }
                write!(output, "</{tag}>").unwrap();
            }
        }
        output.push_str("</runtime-permissions>");
        Ok(output.into_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::permissions::RuntimePermission;

    #[test]
    fn nullable_runtime_names_are_distinct_and_fail_original_attribute_writes() {
        let root = aim_android_xml::read(b"<runtime-permissions><package><permission name='old' granted='true' flags='1'/></package><package name=''/><package><permission name='last' granted='false' flags='2'/></package><shared-user/><shared-user name=''/></runtime-permissions>").unwrap();
        let state = RuntimePermissions::parse(&root).unwrap();
        assert_eq!(
            state
                .packages
                .iter()
                .map(|(name, _)| name.as_deref())
                .collect::<Vec<_>>(),
            [None, Some("")]
        );
        assert_eq!(state.packages[0].1[0].name.as_deref(), Some("last"));
        assert_eq!(
            state.shared_users,
            [(None, vec![]), (Some("".into()), vec![])]
        );
        assert!(state.serialize().is_err());
        let state = RuntimePermissions::parse(&aim_android_xml::read(b"<runtime-permissions><package name='p'><permission granted='true' flags='1'/></package></runtime-permissions>").unwrap()).unwrap();
        assert_eq!(state.packages[0].1[0].name, None);
        let mut migrated = crate::package::owner::legacy_permissions::Migration::default();
        migrated.read_runtime(0, &state.packages[0].1).unwrap();
        assert_eq!(migrated.permissions(0).unwrap()[0].name, None);
        assert!(state.serialize().is_err());
    }

    #[test]
    fn runtime_owner_maps_replace_duplicates_and_keep_hash_collision_slots() {
        let bytes = b"<runtime-permissions><package name='BB'><permission name='old' granted='true' flags='1'/></package><package name='Aa'/><package name='z'/><package name='BB'><permission name='last' granted='false' flags='2'/></package><shared-user name='group'><permission name='old' granted='true' flags='1'/></shared-user><shared-user name='group'/></runtime-permissions>";
        let state = RuntimePermissions::parse(&aim_android_xml::read(bytes).unwrap()).unwrap();
        assert_eq!(
            state
                .packages
                .iter()
                .map(|(name, _)| name.as_deref())
                .collect::<Vec<_>>(),
            [Some("z"), Some("BB"), Some("Aa")]
        );
        assert_eq!(state.packages[1].1[0].name.as_deref(), Some("last"));
        assert_eq!(state.shared_users, [(Some("group".into()), vec![])]);
    }

    #[test]
    fn runtime_writer_retains_order_empty_states_and_revokes_one_time_grants() {
        let source = RuntimePermissions {
            version: -1,
            fingerprint: Some("<&\"\t\n\r".into()),
            packages: vec![
                (
                    Some("p".into()),
                    vec![
                        RuntimePermission {
                            name: Some("ordinary".into()),
                            granted: true,
                            flags: 17,
                        },
                        RuntimePermission {
                            name: Some("one-time".into()),
                            granted: true,
                            flags: 1 << 16,
                        },
                    ],
                ),
                (Some("empty".into()), vec![]),
            ],
            shared_users: vec![(Some("group".into()), vec![])],
        };
        let restored = RuntimePermissions::parse(
            &aim_android_xml::read(&source.serialize().unwrap()).unwrap(),
        )
        .unwrap();
        let mut expected = source.clone();
        expected.packages[0].1[1].granted = false;
        assert_eq!(restored, expected);
        assert!(
            RuntimePermissions {
                fingerprint: Some("bad\u{1}".into()),
                ..Default::default()
            }
            .serialize()
            .is_err()
        );
    }

    #[test]
    fn negative_flags_keep_original_unsigned_hex_write_and_signed_read_failure() {
        let state = RuntimePermissions {
            packages: vec![(
                Some("p".into()),
                vec![RuntimePermission {
                    name: Some("permission".into()),
                    granted: false,
                    flags: -1,
                }],
            )],
            ..Default::default()
        };
        let bytes = state.serialize().unwrap();
        assert!(
            std::str::from_utf8(&bytes)
                .unwrap()
                .contains("flags=\"ffffffff\"")
        );
        assert!(RuntimePermissions::parse(&aim_android_xml::read(&bytes).unwrap()).is_err());
    }
}
