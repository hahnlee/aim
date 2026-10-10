//! DomainVerificationPersistence.readFromXml, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{Package, SectionError, State, User, boolean, legacy_read::Sections, number, put};
use crate::package::{intent_filter::UriRelativeFilterGroup, settings::ReadError, string};
use aim_android_xml::{Element, pull::Reader};

pub struct ReadResult {
    pub state: State,
    pub diagnostics: Vec<SectionError>,
}

impl State {
    /// Parse detached active/restored maps before the live owner merges them.
    /// UUID behavior comes from the pinned process policy owner, not a default.
    pub fn read_events(
        reader: &mut Reader<'_>,
        mut parse_uuid: impl FnMut(&str) -> Result<String, ReadError>,
    ) -> Result<ReadResult, ReadError> {
        let mut sections = Sections::new(reader);
        let mut state = State::default();
        sections.children();
        while let Some(start) = sections.next_named(None) {
            let target = match start.name.as_str() {
                "active" => &mut state.active,
                "restored" => &mut state.restored,
                _ => continue,
            };
            sections.children();
            while let Some(start) = sections.next_named(Some("package-state")) {
                if let Some(package) = package(&mut sections, &start, &mut parse_uuid)? {
                    put(target, package.name.clone(), package, |p| &p.name);
                }
            }
        }
        Ok(ReadResult {
            state,
            diagnostics: sections.errors,
        })
    }
}

fn package(
    sections: &mut Sections<'_, '_>,
    start: &Element,
    parse_uuid: &mut impl FnMut(&str) -> Result<String, ReadError>,
) -> Result<Option<Package>, ReadError> {
    let (Some(name), Some(id)) = (string(start, "packageName"), string(start, "id")) else {
        return Ok(None);
    };
    if name.is_empty() || id.is_empty() {
        return Ok(None);
    }
    let mut package = Package {
        name,
        id: parse_uuid(&id)?,
        has_auto_verify_domains: boolean(start, "hasAutoVerifyDomains", false),
        signature: string(start, "signature"),
        domains: vec![],
        users: vec![],
        uri_relative_filter_groups: vec![],
    };
    sections.children();
    while let Some(start) = sections.next_named(None) {
        match start.name.as_str() {
            "state" => {
                sections.children();
                while let Some(domain) = sections.next_named(Some("domain")) {
                    let name = string(&domain, "name");
                    let state = number(&domain, "state", 0);
                    put(
                        &mut package.domains,
                        name.clone(),
                        (name, state),
                        |(name, _)| name,
                    );
                }
            }
            "user-states" => users(sections, &mut package.users),
            "uri-relative-filter-groups" => {
                sections.children();
                while let Some(domain) = sections.next_named(Some("domain")) {
                    let name = string(&domain, "name");
                    let mut groups = Vec::new();
                    sections.children();
                    while let Some(group) = sections.next_named(Some("uri-relative-filter-group")) {
                        let mut parsed = UriRelativeFilterGroup::new(number(&group, "action", -1));
                        sections.children();
                        while let Some(filter) = sections.next_named(Some("uri-relative-filter")) {
                            if let Some(value) = string(&filter, "filter") {
                                parsed.add(
                                    number(&filter, "uri-part", -1),
                                    number(&filter, "pattern-type", -1),
                                    &value,
                                );
                            }
                        }
                        groups.push(parsed);
                    }
                    put(
                        &mut package.uri_relative_filter_groups,
                        name.clone(),
                        (name, groups),
                        |(name, _)| name,
                    );
                }
            }
            _ => {}
        }
    }
    Ok(Some(package))
}

fn users(sections: &mut Sections<'_, '_>, users: &mut Vec<User>) {
    sections.children();
    while let Some(start) = sections.next_named(Some("user-state")) {
        let id = number(&start, "userId", -1);
        if id == -1 {
            continue;
        }
        let mut hosts = Vec::new();
        sections.children();
        while let Some(start) = sections.next_named(None) {
            if start.name == "enabled-hosts" {
                sections.children();
                while let Some(host) = sections.next_named(Some("host")) {
                    if let Some(name) = string(&host, "name").filter(|name| !name.is_empty())
                        && !hosts.contains(&name)
                    {
                        hosts.push(name);
                    }
                }
            }
        }
        // The shared section cursor has moved by now; keep start attributes.
        put(
            users,
            id,
            User {
                id,
                allow_link_handling: boolean(&start, "allowLinkHandling", false),
                enabled_hosts: hosts,
            },
            |user| &user.id,
        );
        users.sort_by_key(|user| user.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ast_import_keeps_existing_state_when_later_package_uuid_fails() {
        let mut state = State::default();
        let good = aim_android_xml::read(b"<domain-verifications><active><package-state packageName='old' id='00000000-0000-0000-0000-000000000001'/></active></domain-verifications>").unwrap();
        state.read(&good).unwrap();
        let original = state.clone();
        let bad = aim_android_xml::read(b"<domain-verifications><active><package-state packageName='new' id='00000000-0000-0000-0000-000000000002'/><package-state packageName='bad' id='bad'/></active></domain-verifications>").unwrap();
        assert!(state.read(&bad).is_err());
        assert_eq!(state, original);
    }

    #[test]
    fn detached_maps_preserve_nested_record_cursor_and_parse_failure_boundary() {
        let bytes = b"<domain-verifications><unknown><active><unknown><package-state packageName='p' id='1-1-1-1-1'><state><unknown><domain name='example' state='2'/></unknown></state><user-states><user-state userId='1' allowLinkHandling='true'><enabled-hosts><host name='example'/></enabled-hosts></user-state></user-states></package-state></unknown></active></unknown></domain-verifications>";
        let mut reader = Reader::new(bytes).unwrap();
        reader.next().unwrap();
        let result = State::read_events(&mut reader, |id| {
            super::super::uuid::parse(id, true).map_err(ReadError::File)
        })
        .unwrap();
        assert_eq!(
            result.state.active[0].domains,
            [(Some("example".into()), 2)]
        );
        assert!(result.state.active[0].users[0].allow_link_handling);
        assert_eq!(
            result.state.active[0].id,
            "00000001-0001-0001-0001-000000000001"
        );
        let mut reader = Reader::new(b"<domain-verifications><active><package-state packageName='p' id='bad'/></active></domain-verifications>").unwrap();
        reader.next().unwrap();
        assert!(matches!(
            State::read_events(&mut reader, |id| super::super::uuid::parse(id, true)
                .map_err(ReadError::File)),
            Err(ReadError::File(_))
        ));
    }
}
