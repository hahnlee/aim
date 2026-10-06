//! DomainVerificationService.writeSettings(..., false, USER_ALL) at the pin.
//! Ports AOSP android-16.0.0_r1, Apache License 2.0.
use super::{attribute, element};
use crate::package::domain_verification::{Package, State};
use aim_android_xml::{Element, Node, Value};

pub fn replace(original: &Element, desired: &State) -> Result<Element, String> {
    let mut root = original.clone();
    let mut current = element("domain-verifications");
    for (tag, packages) in [("active", &desired.active), ("restored", &desired.restored)] {
        let mut section = element(tag);
        for p in packages {
            section.content.push(Node::Element(package(p)));
        }
        current.content.push(Node::Element(section));
    }
    let mut legacy = element("domain-verifications-legacy");
    for (name, users) in &desired.legacy {
        let mut section = element("user-states");
        text(&mut section, "packageName", name.as_deref());
        for (id, state) in users {
            let mut user = element("user-state");
            number(&mut user, "userId", *id);
            number(&mut user, "state", *state);
            section.content.push(Node::Element(user));
        }
        legacy.content.push(Node::Element(section));
    }
    for mut fresh in [current, legacy] {
        let old: Vec<_> = original
            .children()
            .filter(|e| e.name == fresh.name)
            .collect();
        if old.len() > 1 {
            return Err("duplicate domain persistence section".into());
        }
        if let Some(old) = old.first() {
            preserve(old, &mut fresh);
        }
        root.content
            .retain(|n| !matches!(n, Node::Element(e) if e.name == fresh.name));
        root.content.push(Node::Element(fresh));
    }
    // Validate the entire document before opening any writable file.
    let parsed = crate::package::settings::Settings::parse(&root)?.domain_verification;
    if parsed.active.len() != desired.active.len()
        || parsed.restored.len() != desired.restored.len()
    {
        return Err("invalid or duplicate domain persistence identity".into());
    }
    Ok(root)
}
fn package(p: &Package) -> Element {
    let mut e = element("package-state");
    text(&mut e, "packageName", Some(&p.name));
    text(&mut e, "id", Some(&p.id));
    if p.has_auto_verify_domains {
        attribute(&mut e, "hasAutoVerifyDomains", Some(Value::Bool(true)));
    }
    text(&mut e, "signature", p.signature.as_deref());
    if !p.domains.is_empty() {
        let mut states = element("state");
        for (host, state) in &p.domains {
            let mut domain = element("domain");
            text(&mut domain, "name", host.as_deref());
            number(&mut domain, "state", *state);
            states.content.push(Node::Element(domain));
        }
        e.content.push(Node::Element(states));
    }
    if !p.users.is_empty() {
        let mut users = element("user-states");
        for u in &p.users {
            let mut user = element("user-state");
            number(&mut user, "userId", u.id);
            if u.allow_link_handling {
                attribute(&mut user, "allowLinkHandling", Some(Value::Bool(true)));
            }
            if !u.enabled_hosts.is_empty() {
                let mut hosts = element("enabled-hosts");
                for host in &u.enabled_hosts {
                    let mut h = element("host");
                    text(&mut h, "name", Some(host));
                    hosts.content.push(Node::Element(h));
                }
                user.content.push(Node::Element(hosts));
            }
            users.content.push(Node::Element(user));
        }
        e.content.push(Node::Element(users));
    }
    if !p.uri_relative_filter_groups.is_empty() {
        let mut groups = element("uri-relative-filter-groups");
        for (host, values) in &p.uri_relative_filter_groups {
            if values.is_empty() {
                continue;
            }
            let mut domain = element("domain");
            text(&mut domain, "name", host.as_deref());
            for group in values {
                let mut g = element("uri-relative-filter-group");
                number(&mut g, "action", group.action);
                for f in &group.filters {
                    let mut filter = element("uri-relative-filter");
                    number(&mut filter, "uri-part", f.uri_part);
                    number(&mut filter, "pattern-type", f.pattern_type);
                    text(&mut filter, "filter", f.filter.as_deref());
                    g.content.push(Node::Element(filter));
                }
                domain.content.push(Node::Element(g));
            }
            groups.content.push(Node::Element(domain));
        }
        e.content.push(Node::Element(groups));
    }
    e
}
fn text(e: &mut Element, key: &str, value: Option<&str>) {
    attribute(e, key, value.map(|v| Value::String(v.into())));
}
fn number(e: &mut Element, key: &str, value: i32) {
    if value != -1 {
        attribute(e, key, Some(Value::Int(value)));
    }
}

// Preserve future fields on surviving records. Removed records remain removed.
fn schema(tag: &str) -> (&'static [&'static str], &'static [&'static str]) {
    match tag {
        "domain-verifications" => (&[], &["active", "restored"]),
        "domain-verifications-legacy" => (&[], &["user-states"]),
        "active" | "restored" => (&[], &["package-state"]),
        "package-state" => (
            &["packageName", "id", "hasAutoVerifyDomains", "signature"],
            &["state", "user-states", "uri-relative-filter-groups"],
        ),
        "state" | "uri-relative-filter-groups" => (&[], &["domain"]),
        "domain" => (&["name", "state"], &["uri-relative-filter-group"]),
        "user-states" => (&["packageName"], &["user-state"]),
        "user-state" => (
            &["userId", "state", "allowLinkHandling"],
            &["enabled-hosts"],
        ),
        "enabled-hosts" => (&[], &["host"]),
        "host" => (&["name"], &[]),
        "uri-relative-filter-group" => (&["action"], &["uri-relative-filter"]),
        "uri-relative-filter" => (&["uri-part", "pattern-type", "filter"], &[]),
        _ => (&[], &[]),
    }
}
fn identity(e: &Element) -> (String, Option<String>) {
    let key = match e.name.as_str() {
        "package-state" | "user-states" => e.string("packageName").map(|v| v.into_owned()),
        "domain" | "host" => e.string("name").map(|v| v.into_owned()),
        "user-state" => e.int("userId").ok().flatten().map(|v| v.to_string()),
        "uri-relative-filter" => Some(format!(
            "{:?}/{:?}/{:?}",
            e.int("uri-part"),
            e.int("pattern-type"),
            e.string("filter")
        )),
        _ => None,
    };
    (e.name.clone(), key)
}
fn preserve(old: &Element, fresh: &mut Element) {
    let (attrs, children) = schema(&fresh.name);
    for (name, value) in &old.attrs {
        if !attrs.contains(&name.as_str()) {
            fresh.attrs.push((name.clone(), value.clone()));
        }
    }
    let mut matched = std::collections::BTreeSet::new();
    for node in &mut fresh.content {
        if let Node::Element(e) = node {
            if let Some((i, source)) = old
                .children()
                .enumerate()
                .find(|(i, source)| !matched.contains(i) && identity(source) == identity(e))
            {
                matched.insert(i);
                preserve(source, e);
            }
        }
    }
    // A vanished data container can still own future metadata. Keep its
    // unknown payload without retaining removed package/user/filter rows.
    for source in old.children() {
        if matches!(
            source.name.as_str(),
            "active"
                | "restored"
                | "state"
                | "user-states"
                | "enabled-hosts"
                | "uri-relative-filter-groups"
        ) && !fresh.children().any(|e| identity(e) == identity(source))
        {
            let mut shell = element(&source.name);
            preserve(source, &mut shell);
            if !shell.attrs.is_empty()
                || shell.content.iter().any(|n| matches!(n, Node::Element(_)))
            {
                fresh.content.push(Node::Element(shell));
            }
        }
    }
    for node in &old.content {
        if !matches!(node, Node::Element(e) if children.contains(&e.name.as_str())) {
            fresh.content.push(node.clone());
        }
    }
}
