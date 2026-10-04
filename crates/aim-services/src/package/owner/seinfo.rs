//! SELinuxMMAC policy and label composition, android-16.0.0_r1 (#838).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
mod sort;
use crate::package::sign::SigningDetails;
use aim_android_xml::Element;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    certificates: BTreeSet<Vec<u8>>,
    global: Option<String>,
    packages: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    rules: Vec<Rule>,
}

/// UNKNOWN is the original SigningDetails sentinel, not absent scan metadata.
#[derive(Clone, Copy)]
pub enum Signing<'a> {
    Unknown,
    Known(&'a SigningDetails),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Partition {
    Data,
    System,
    SystemExt,
    Product,
    Vendor,
    Oem,
    Odm,
}

impl Partition {
    pub fn from_state(state: &crate::package::model::StateFlags) -> Self {
        if state.system_ext {
            Self::SystemExt
        } else if state.product {
            Self::Product
        } else if state.vendor {
            Self::Vendor
        } else if state.oem {
            Self::Oem
        } else if state.odm {
            Self::Odm
        } else if state.system {
            Self::System
        } else {
            Self::Data
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Data => "",
            Self::System => "system",
            Self::SystemExt => "system_ext",
            Self::Product => "product",
            Self::Vendor => "vendor",
            Self::Oem => "oem",
            Self::Odm => "odm",
        }
    }
}

impl Policy {
    /// The original's initial state before any policy was successfully read.
    pub fn unread() -> Self {
        Self { rules: Vec::new() }
    }

    /// Required platform policy, followed by the original optional partition files.
    pub fn load(image: &Path) -> Result<Self, String> {
        let mut roots = Vec::new();
        for (index, path) in [
            "system/etc/selinux/plat_mac_permissions.xml",
            "system_ext/etc/selinux/system_ext_mac_permissions.xml",
            "product/etc/selinux/product_mac_permissions.xml",
            "vendor/etc/selinux/vendor_mac_permissions.xml",
            "odm/etc/selinux/odm_mac_permissions.xml",
        ]
        .iter()
        .enumerate()
        {
            let path = image.join(path);
            match fs::read(&path) {
                Ok(bytes) => roots.push(
                    aim_android_xml::read(&bytes)
                        .map_err(|e| format!("{}: {e}", path.display()))?,
                ),
                Err(e) if index != 0 && e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
        Self::parse(&roots)
    }

    /// Validate every file before returning a policy; no partial policy is published.
    pub fn parse(roots: &[Element]) -> Result<Self, String> {
        let mut rules = Vec::new();
        for root in roots {
            if root.name != "policy" {
                return Err("expected mac-permissions policy".into());
            }
            for signer in root.children().filter(|e| e.name == "signer") {
                let mut rule = Rule {
                    certificates: BTreeSet::new(),
                    global: None,
                    packages: BTreeMap::new(),
                };
                if let Some(cert) = signer.bytes_hex("signature")? {
                    rule.certificates.insert(cert);
                }
                for child in signer.children() {
                    match child.name.as_str() {
                        "cert" => {
                            rule.certificates.insert(
                                child
                                    .bytes_hex("signature")?
                                    .ok_or("missing policy certificate")?,
                            );
                        }
                        "seinfo" => {
                            let value = value(child, "value")?;
                            if rule.global.as_ref().is_some_and(|old| old != &value) {
                                return Err("conflicting global seinfo".into());
                            }
                            rule.global = Some(value);
                        }
                        "package" => {
                            for info in child.children().filter(|e| e.name == "seinfo") {
                                let name = value(child, "name")?;
                                let value = value(info, "value")?;
                                if rule
                                    .packages
                                    .insert(name, value.clone())
                                    .is_some_and(|old| old != value)
                                {
                                    return Err("conflicting package seinfo".into());
                                }
                            }
                        }
                        _ => {}
                    }
                }
                if rule.certificates.is_empty() {
                    return Err("policy signer has no certificate".into());
                }
                if rule.global.is_some() == !rule.packages.is_empty() {
                    return Err("policy needs global seinfo xor package mappings".into());
                }
                rules.push(rule);
            }
        }
        sort_rules(&mut rules)?;
        Ok(Self { rules })
    }

    pub fn label(
        &self,
        name: &str,
        signing: Signing<'_>,
        privileged: bool,
        target_sdk: i32,
        partition: Partition,
    ) -> String {
        let base = self
            .rules
            .iter()
            .find_map(|rule| {
                if let Signing::Known(signing) = signing {
                    let exact = signing.signatures.len() == rule.certificates.len()
                        && signing
                            .signatures
                            .iter()
                            .all(|c| rule.certificates.contains(c))
                        && rule
                            .certificates
                            .iter()
                            .all(|c| signing.signatures.contains(c));
                    if !exact
                        && (rule.certificates.len() != 1
                            || !has_certificate(signing, rule.certificates.first().unwrap()))
                    {
                        return None;
                    }
                }
                rule.packages.get(name).or(rule.global.as_ref())
            })
            .map(String::as_str)
            .unwrap_or("default");
        let mut label = base.to_owned();
        if privileged {
            label.push_str(":privapp");
        }
        label.push_str(&format!(":targetSdkVersion={target_sdk}"));
        if partition != Partition::Data {
            label.push_str(":partition=");
            label.push_str(partition.label());
        }
        label
    }
}

/// The bootstrap owner supplies both compatibility decisions. A nonempty shared
/// UID supplies its boot-fixed SDK, which takes precedence over compatibility.
pub fn target_sdk(
    declared: i32,
    shared: Option<i32>,
    latest_changes: bool,
    r_changes: bool,
) -> i32 {
    if let Some(shared) = shared {
        shared
    } else if latest_changes {
        declared.max(10000)
    } else if r_changes {
        declared.max(30)
    } else {
        declared
    }
}

fn sort_rules(rules: &mut Vec<Rule>) -> Result<(), String> {
    use std::cmp::Ordering;
    let order = sort::sort(rules.len(), |a, b| {
        let (a, b) = (&rules[a], &rules[b]);
        let order = a.packages.is_empty().cmp(&b.packages.is_empty());
        if order == Ordering::Equal
            && a.certificates == b.certificates
            && (a.global.is_some() || a.packages.keys().any(|name| b.packages.contains_key(name)))
        {
            return Err("duplicate mac-permissions policy".into());
        }
        Ok(order)
    })?;
    *rules = order.into_iter().map(|i| rules[i].clone()).collect();
    Ok(())
}

fn value(element: &Element, name: &str) -> Result<String, String> {
    let value = element
        .string(name)
        .ok_or_else(|| format!("missing policy {name}"))?;
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
    {
        return Err(format!("invalid policy {name}"));
    }
    Ok(value.into_owned())
}

fn has_certificate(signing: &SigningDetails, certificate: &[u8]) -> bool {
    signing
        .past_signing_certificates
        .as_ref()
        .is_some_and(|past| {
            past.iter()
                .take(past.len().saturating_sub(1))
                .any(|(cert, _)| cert == certificate)
        })
        || (signing.signatures.len() == 1 && signing.signatures[0] == certificate)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(text: &str) -> Result<Policy, String> {
        Policy::parse(&[aim_android_xml::read(text.as_bytes())?])
    }
    fn signing(certificates: &[&[u8]], past: Option<Vec<(Vec<u8>, i32)>>) -> SigningDetails {
        SigningDetails {
            unknown: false,
            current_flags: Vec::new(),
            signatures: certificates.iter().map(|c| c.to_vec()).collect(),
            scheme_version: 3,
            public_keys: vec![],
            past_signing_certificates: past,
        }
    }
    #[test]
    fn large_policy_sets_preserve_specificity_and_acceptance() {
        for count in [32, 33, 64, 65, 257, 1024] {
            let mut xml = String::from("<policy>");
            for index in 0..count {
                xml.push_str(&format!("<signer signature='{index:04x}'>"));
                if index % 2 == 0 {
                    xml.push_str(&format!("<seinfo value='label{index}'/>"));
                } else {
                    xml.push_str(&format!(
                        "<package name='app'><seinfo value='label{index}'/></package>"
                    ));
                }
                xml.push_str("</signer>");
            }
            xml.push_str("</policy>");
            let policy = parse(&xml).unwrap();
            assert_eq!(
                policy.label("app", Signing::Unknown, false, 36, Partition::Data),
                "label1:targetSdkVersion=36"
            );
            let certificate = [((count - 1) >> 8) as u8, (count - 1) as u8];
            assert_eq!(
                policy.label(
                    "app",
                    Signing::Known(&signing(&[&certificate], None)),
                    false,
                    36,
                    Partition::Data
                ),
                format!("label{}:targetSdkVersion=36", count - 1)
            );
        }
    }

    #[test]
    fn policy_validation_specificity_rotation_unknown_and_label_suffixes() {
        let policy = parse("<policy><signer signature='01'><seinfo value='platform'/></signer><signer signature='01'><package name='app'><seinfo value='specific'/></package></signer></policy>").unwrap();
        let current = signing(&[&[1]], None);
        assert_eq!(
            policy.label("app", Signing::Known(&current), true, 36, Partition::System),
            "specific:privapp:targetSdkVersion=36:partition=system"
        );
        assert_eq!(
            policy.label(
                "other",
                Signing::Known(&current),
                false,
                29,
                Partition::Data
            ),
            "platform:targetSdkVersion=29"
        );
        let rotated = signing(&[&[2]], Some(vec![(vec![1], 0), (vec![2], 0)]));
        assert_eq!(
            policy.label(
                "app",
                Signing::Known(&rotated),
                false,
                30,
                Partition::Vendor
            ),
            "specific:targetSdkVersion=30:partition=vendor"
        );
        let wrong = signing(&[&[2]], None);
        assert_eq!(
            policy.label("app", Signing::Known(&wrong), false, 30, Partition::Data),
            "default:targetSdkVersion=30"
        );
        assert_eq!(
            policy.label("app", Signing::Unknown, false, 36, Partition::Data),
            "specific:targetSdkVersion=36"
        );
        let multi = parse("<policy><signer signature='01'><cert signature='02'/><seinfo value='multi'/></signer></policy>").unwrap();
        assert_eq!(
            multi.label(
                "app",
                Signing::Known(&signing(&[&[2], &[1]], None)),
                false,
                36,
                Partition::Data
            ),
            "multi:targetSdkVersion=36"
        );
        assert_eq!(
            multi.label("app", Signing::Known(&rotated), false, 36, Partition::Data),
            "default:targetSdkVersion=36"
        );
        for text in [
            "<policy><signer><seinfo value='x'/></signer></policy>",
            "<policy><signer signature='0'><seinfo value='x'/></signer></policy>",
            "<policy><signer signature='01'/></policy>",
            "<policy><signer signature='01'><seinfo value='x:y'/></signer></policy>",
            "<policy><signer signature='01'><seinfo value='x'/><package name='a'><seinfo value='y'/></package></signer></policy>",
            "<policy><signer signature='01'><seinfo value='x'/></signer><signer signature='01'><seinfo value='x'/></signer></policy>",
        ] {
            assert!(parse(text).is_err());
        }
        assert_eq!(target_sdk(28, None, false, false), 28);
        assert_eq!(target_sdk(28, None, false, true), 30);
        assert_eq!(target_sdk(36, None, true, true), 10000);
        assert_eq!(target_sdk(36, Some(28), true, true), 28);
    }
}
