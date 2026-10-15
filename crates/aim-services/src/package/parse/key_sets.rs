//! ParsingPackageUtils.parseKeySets at android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::{Error, Result, package::Package, parcel::ArraySet};
use crate::package::sign::{canonical_public_keys, deserialize_public_key, serialize_public_keys};
use aim_apps::res::Element;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
struct Keys {
    public: HashMap<Option<String>, Vec<u8>>,
    defined: Vec<(Option<String>, Vec<Option<String>>)>,
    improper: HashSet<Option<String>>,
    upgrades: Vec<Option<String>>,
}

pub(super) fn parse(
    pkg: &mut Package,
    element: &Element,
    attr: impl Fn(&Element, &str) -> Option<String>,
) -> Result<()> {
    let mut keys = Keys::default();
    keys.walk(&element.children, None, &attr)?;
    if keys
        .defined
        .iter()
        .any(|(name, _)| keys.public.contains_key(name))
    {
        return Err(Error::Parse(
            "key-set and public-key names must be distinct".into(),
        ));
    }
    let mut mapping = pkg.key_set_mapping.clone();
    for (name, names) in keys.defined {
        if names.is_empty() || keys.improper.contains(&name) {
            continue;
        }
        let name = name.ok_or_else(|| Error::Parse("public-key outside a named key-set".into()))?;
        let mut public = mapping
            .get(&name)
            .into_iter()
            .flatten()
            .map(deserialize_public_key)
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(Error::Parse)?;
        public.extend(names.iter().map(|name| keys.public[name].clone()));
        mapping.put(&name, serialize_public_keys(&public).map_err(Error::Parse)?);
    }
    let mut upgrades = ArraySet::default();
    for name in keys.upgrades {
        let Some(name) = name.filter(|name| mapping.contains(name)) else {
            return Err(Error::Parse(
                "manifest does not define all upgrade-key-sets".into(),
            ));
        };
        upgrades.add(&name);
    }
    pkg.key_set_mapping = mapping;
    pkg.upgrade_key_sets = upgrades;
    Ok(())
}

impl Keys {
    fn walk(
        &mut self,
        elements: &[Element],
        current: Option<&str>,
        attr: &impl Fn(&Element, &str) -> Option<String>,
    ) -> Result<()> {
        for e in elements {
            match e.name.as_str() {
                "key-set" => {
                    if current.is_some() {
                        return Err(Error::Parse("improperly nested key-set".into()));
                    }
                    let name = attr(e, "name");
                    if let Some((_, members)) =
                        self.defined.iter_mut().find(|(old, _)| old == &name)
                    {
                        members.clear();
                    } else {
                        self.defined.push((name.clone(), Vec::new()));
                    }
                    self.walk(&e.children, name.as_deref(), attr)?;
                }
                "public-key" => {
                    let current = current
                        .ok_or_else(|| Error::Parse("public-key outside a named key-set".into()))?;
                    let name = attr(e, "name");
                    if let Some(encoded) = attr(e, "value") {
                        let parsed = match base64(&encoded) {
                            Some(der) => match canonical_public_keys(&[der]) {
                                Ok(mut keys) => Some(keys.remove(0)),
                                Err(error) if error.contains("unsupported public-key curve") => {
                                    return Err(Error::Unsupported(error));
                                }
                                Err(_) => None,
                            },
                            None => None,
                        };
                        let Some(public) = parsed else {
                            self.improper.insert(Some(current.into()));
                            continue;
                        };
                        if self.public.get(&name).is_some_and(|old| old != &public) {
                            return Err(Error::Parse(
                                "public-key value conflicts with previous definition".into(),
                            ));
                        }
                        self.public.insert(name.clone(), public);
                    } else if !self.public.contains_key(&name) {
                        return Err(Error::Parse(
                            "public-key must define a value on first use".into(),
                        ));
                    }
                    let members = &mut self
                        .defined
                        .iter_mut()
                        .find(|(name, _)| name.as_deref() == Some(current))
                        .unwrap()
                        .1;
                    if !members.contains(&name) {
                        members.push(name);
                    }
                    members.sort_by_key(|name| {
                        name.as_deref().map(super::parcel::java_hash).unwrap_or(0)
                    });
                }
                "upgrade-key-set" => {
                    let name = attr(e, "name");
                    if !self.upgrades.contains(&name) {
                        self.upgrades.push(name);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
}

/// android.util.Base64.DEFAULT: skip non-alphabet bytes; padding is optional
/// but, once encountered, must finish the current unit and then the stream.
fn base64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut state, mut value) = (0, 0u32);
    for byte in text.bytes() {
        let digit = match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        };
        if let Some(digit) = digit {
            if state >= 4 {
                return None;
            }
            value = (value << 6) | u32::from(digit);
            state += 1;
            if state == 4 {
                out.extend_from_slice(&[(value >> 16) as u8, (value >> 8) as u8, value as u8]);
                state = 0;
            }
        } else if byte == b'=' {
            match state {
                2 => {
                    out.push((value >> 4) as u8);
                    state = 4;
                }
                3 => {
                    out.extend_from_slice(&[(value >> 10) as u8, (value >> 2) as u8]);
                    state = 5;
                }
                4 => state = 5,
                _ => return None,
            }
        }
    }
    match state {
        0 | 5 => {}
        2 => out.push((value >> 4) as u8),
        3 => out.extend_from_slice(&[(value >> 10) as u8, (value >> 2) as u8]),
        _ => return None,
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_apps::res::{Attr, Value};
    use p256::elliptic_curve::sec1::ToEncodedPoint;

    fn node(name: &str, attributes: &[(&str, String)], children: Vec<Element>) -> Element {
        Element {
            name: name.into(),
            attrs: attributes
                .iter()
                .map(|(name, value)| Attr {
                    ns: super::super::attrs::ANDROID.into(),
                    name: (*name).into(),
                    id: 0,
                    value: Value::String(value.clone()),
                    kind: 3,
                    data: 0,
                })
                .collect(),
            children,
        }
    }

    fn attr(element: &Element, name: &str) -> Option<String> {
        element.attrs.iter().find(|a| a.name == name).and_then(|a| {
            if let Value::String(value) = &a.value {
                Some(value.clone())
            } else {
                None
            }
        })
    }

    fn key(scalar: u8) -> (Vec<u8>, String) {
        let mut der = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 2, 1, 0x06, 8, 0x2a,
            0x86, 0x48, 0xce, 0x3d, 3, 1, 7, 3, 0x42, 0,
        ];
        let secret = p256::SecretKey::from_slice(&[scalar; 32]).unwrap();
        der.extend_from_slice(secret.public_key().to_encoded_point(false).as_bytes());
        let mut element = aim_android_xml::Element {
            name: "key".into(),
            attrs: Vec::new(),
            content: Vec::new(),
        };
        element.attrs.push((
            "value".into(),
            aim_android_xml::Value::BytesBase64(der.clone()),
        ));
        (der, element.string("value").unwrap().into_owned())
    }

    #[test]
    fn names_reuse_nullable_public_names_and_parcel_roundtrip() {
        let (der, encoded) = key(1);
        let manifest = node(
            "key-sets",
            &[],
            vec![
                node(
                    "key-set",
                    &[("name", "z".into())],
                    vec![node("public-key", &[("value", encoded)], vec![])],
                ),
                node(
                    "key-set",
                    &[("name", "a".into())],
                    vec![node("public-key", &[], vec![])],
                ),
                node("upgrade-key-set", &[("name", "a".into())], vec![]),
                node("upgrade-key-set", &[("name", "a".into())], vec![]),
            ],
        );
        let mut package = Package::new("example.app", "/base.apk", "/");
        parse(&mut package, &manifest, attr).unwrap();
        assert_eq!(
            package.key_set_mapping.keys().collect::<Vec<_>>(),
            ["a", "z"]
        );
        assert_eq!(package.upgrade_key_sets.iter().collect::<Vec<_>>(), ["a"]);
        let entry = package.to_cache_entry();
        let restored = crate::package::pkg::AndroidPackage::read_cache_entry(&entry.bytes).unwrap();
        let mapping = restored.key_set_mapping.unwrap();
        assert_eq!(mapping.len(), 2);
        for (name, keys) in mapping {
            assert!(matches!(name.as_deref(), Some("a" | "z")));
            let keys = keys.unwrap();
            assert_eq!(keys.len(), 1);
            assert_eq!(
                deserialize_public_key(keys[0].as_ref().unwrap()).unwrap(),
                der
            );
        }
    }

    #[test]
    fn invalid_sets_are_omitted_and_upgrade_requires_a_retained_definition() {
        let (_, encoded) = key(1);
        let mut manifest = node(
            "key-sets",
            &[],
            vec![
                node("key-set", &[("name", "empty".into())], vec![]),
                node(
                    "key-set",
                    &[("name", "invalid".into())],
                    vec![
                        node(
                            "public-key",
                            &[("name", "one".into()), ("value", encoded)],
                            vec![],
                        ),
                        node(
                            "public-key",
                            &[("name", "bad".into()), ("value", "not DER".into())],
                            vec![],
                        ),
                    ],
                ),
            ],
        );
        let mut package = Package::new("example.app", "/base.apk", "/");
        parse(&mut package, &manifest, attr).unwrap();
        assert!(package.key_set_mapping.is_empty());
        manifest.children.push(node(
            "upgrade-key-set",
            &[("name", "invalid".into())],
            vec![],
        ));
        assert!(parse(&mut package, &manifest, attr).is_err());
        assert!(package.key_set_mapping.is_empty());
    }

    #[test]
    fn rejects_nested_missing_conflicting_and_colliding_names() {
        let (_, one) = key(1);
        let (_, two) = key(2);
        for children in [
            vec![node("public-key", &[("value", one.clone())], vec![])],
            vec![node(
                "key-set",
                &[("name", "set".into())],
                vec![node("key-set", &[("name", "inner".into())], vec![])],
            )],
            vec![node(
                "key-set",
                &[("name", "set".into())],
                vec![node("public-key", &[("name", "missing".into())], vec![])],
            )],
            vec![node(
                "key-set",
                &[("name", "set".into())],
                vec![
                    node(
                        "public-key",
                        &[("name", "one".into()), ("value", one.clone())],
                        vec![],
                    ),
                    node(
                        "public-key",
                        &[("name", "one".into()), ("value", two)],
                        vec![],
                    ),
                ],
            )],
            vec![node(
                "key-set",
                &[("name", "same".into())],
                vec![node(
                    "public-key",
                    &[("name", "same".into()), ("value", one)],
                    vec![],
                )],
            )],
        ] {
            let mut package = Package::new("example.app", "/base.apk", "/");
            assert!(parse(&mut package, &node("key-sets", &[], children), attr).is_err());
            assert!(package.key_set_mapping.is_empty());
        }
    }

    #[test]
    fn base64_default_skips_non_alphabet_and_checks_padding() {
        assert_eq!(base64(" Z!g==\n"), Some(b"f".to_vec()));
        assert_eq!(base64("Zm8"), Some(b"fo".to_vec()));
        for bad in ["Z", "Zg=", "Zg===", "Zm9v=", "Zg==A"] {
            assert_eq!(base64(bad), None);
        }
    }
}
