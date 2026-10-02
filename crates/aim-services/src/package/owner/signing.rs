//! PackageSignatures.writeXml certificate tables at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{attribute, element};
use crate::package::settings::Settings;
use aim_android_xml::{Element, Node, Value};
use std::collections::BTreeSet;

fn without_signatures(mut settings: Settings) -> Settings {
    for package in settings
        .packages
        .iter_mut()
        .chain(&mut settings.disabled_system_packages)
    {
        package.signatures = None;
    }
    for group in &mut settings.shared_users {
        group.signatures = None;
    }
    settings
}
fn persisted(mut settings: Settings) -> Settings {
    for signatures in settings
        .packages
        .iter_mut()
        .chain(&mut settings.disabled_system_packages)
        .filter_map(|p| p.signatures.as_mut())
        .chain(
            settings
                .shared_users
                .iter_mut()
                .filter_map(|g| g.signatures.as_mut()),
        )
    {
        signatures.public_keys = None;
    }
    settings
}

pub(super) fn replace(original: &Element, desired: &Settings) -> Result<Element, String> {
    let previous = Settings::parse(original)?;
    let old = previous
        .packages
        .iter()
        .chain(&previous.disabled_system_packages)
        .map(|p| &p.signatures)
        .chain(previous.shared_users.iter().map(|g| &g.signatures));
    let new = desired
        .packages
        .iter()
        .chain(&desired.disabled_system_packages)
        .map(|p| &p.signatures)
        .chain(desired.shared_users.iter().map(|g| &g.signatures));
    for (old, new) in old.zip(new) {
        if old.as_ref().is_some_and(|s| !s.signatures.is_empty())
            && !new.as_ref().is_some_and(|s| !s.signatures.is_empty())
        {
            return Err("cannot clear retained signing identities".into());
        }
    }
    if without_signatures(previous) != without_signatures(desired.clone()) {
        return Err("signature commit changed unrelated settings (#798)".into());
    }
    let mut root = original.clone();
    let mut certificates = Vec::new();
    let mut seen = BTreeSet::new();
    for node in &mut root.content {
        let Node::Element(owner) = node else {
            continue;
        };
        let signatures = match owner.name.as_str() {
            "package" | "updated-package" => {
                let name = owner
                    .string("name")
                    .ok_or("package record has no name (#803)")?;
                let packages = if owner.name == "package" {
                    &desired.packages
                } else {
                    &desired.disabled_system_packages
                };
                &packages
                    .iter()
                    .find(|p| p.name == name)
                    .ok_or("unmodelled package record (#803)")?
                    .signatures
            }
            "shared-user" => {
                let name = owner
                    .string("name")
                    .ok_or("shared user has no name (#803)")?;
                &desired
                    .shared_users
                    .iter()
                    .find(|g| g.name == name)
                    .ok_or("unmodelled shared user (#803)")?
                    .signatures
            }
            _ => continue,
        };
        let name = owner.string("name").unwrap().into_owned();
        if !seen.insert((owner.name.clone(), name)) {
            return Err("duplicate signature owner (#803)".into());
        }
        let position = owner
            .content
            .iter()
            .position(|n| matches!(n, Node::Element(e) if e.name == "sigs"));
        owner
            .content
            .retain(|n| !matches!(n, Node::Element(e) if e.name == "sigs"));
        if let Some(signatures) = signatures {
            if signatures.signatures.is_empty() {
                return Err("cannot persist unknown signing details".into());
            }
            let mut sigs = element("sigs");
            attribute(
                &mut sigs,
                "count",
                Some(Value::Int(
                    signatures
                        .signatures
                        .len()
                        .try_into()
                        .map_err(|_| "too many signers")?,
                )),
            );
            attribute(
                &mut sigs,
                "schemeVersion",
                Some(Value::Int(signatures.scheme_version)),
            );
            for certificate in &signatures.signatures {
                sigs.content
                    .push(Node::Element(cert(certificate, None, &mut certificates)));
            }
            if let Some(past) = &signatures.past_signatures {
                let mut lineage = element("pastSigs");
                attribute(
                    &mut lineage,
                    "count",
                    Some(Value::Int(
                        past.len().try_into().map_err(|_| "too many past signers")?,
                    )),
                );
                for (certificate, flags) in past {
                    lineage.content.push(Node::Element(cert(
                        certificate,
                        Some(*flags),
                        &mut certificates,
                    )));
                }
                sigs.content.push(Node::Element(lineage));
            }
            owner.content.insert(
                position
                    .unwrap_or(owner.content.len())
                    .min(owner.content.len()),
                Node::Element(sigs),
            );
        }
    }
    if Settings::parse(&root)? != persisted(desired.clone()) {
        return Err("signature document did not preserve desired settings".into());
    }
    Ok(root)
}

fn cert(bytes: &[u8], flags: Option<i32>, certificates: &mut Vec<Vec<u8>>) -> Element {
    let mut cert = element("cert");
    let existing = certificates.iter().position(|c| c == bytes);
    let index = existing.unwrap_or(certificates.len());
    attribute(&mut cert, "index", Some(Value::Int(index as i32)));
    if existing.is_none() {
        certificates.push(bytes.to_vec());
        attribute(&mut cert, "key", Some(Value::BytesHex(bytes.to_vec())));
    }
    if let Some(flags) = flags {
        attribute(&mut cert, "flags", Some(Value::Int(flags)));
    }
    cert
}
