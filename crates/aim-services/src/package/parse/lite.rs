//! Package-name projection of ApkLiteParseUtils at android-16.0.0_r1.
//! Retains every rejection in the flags=0 parse, without full-package validation.
//! Ported from AOSP, Apache License 2.0.
use super::{
    Result,
    attrs::{ANDROID, attr_value},
    cluster, fail,
};
use aim_apps::{apk::Apk, res::Element};
use std::{collections::BTreeMap, path::Path};
pub struct Environment {
    pub sdk: i32,
    pub codenames: Vec<String>,
    pub properties: BTreeMap<String, String>,
}
fn integer(element: &Element, name: &str, default: i32) -> i32 {
    element
        .attrs
        .iter()
        .find(|attr| attr.ns == ANDROID && attr.name == name)
        .filter(|attr| (16..=31).contains(&attr.kind))
        .map_or(default, |attr| attr.data as i32)
}
fn sdk(element: &Element, environment: &Environment) -> Result<()> {
    let min = attr_value(element, ANDROID, "minSdkVersion").filter(|v| !v.is_empty());
    let target = attr_value(element, ANDROID, "targetSdkVersion").filter(|v| !v.is_empty());
    let mut min_version = 1;
    let mut min_code = None;
    if let Some(value) = &min {
        match value.parse::<i32>() {
            Ok(v) => min_version = v,
            Err(_) => min_code = Some(value.as_str()),
        }
    }
    let target_code = match &target {
        Some(value) if value.parse::<i32>().is_err() => Some(value.as_str()),
        None => min_code,
        _ => None,
    };
    if min.is_none() && target_code.is_some() {
        min_code = target_code;
    }
    let matches = |code: &str| {
        environment
            .codenames
            .iter()
            .any(|name| name == code.split('.').next().unwrap_or(code))
    };
    if let Some(code) = target_code {
        if !matches(code) {
            return fail(format!("Requires development platform {code}"));
        }
    }
    match min_code {
        Some(code) if !matches(code) => fail(format!("Requires development platform {code}")),
        None if min_version > environment.sdk => {
            fail(format!("Requires newer sdk version #{min_version}"))
        }
        _ => Ok(()),
    }
}
fn java_split(value: &str) -> Vec<&str> {
    let mut parts: Vec<_> = value.split(',').collect();
    if !value.is_empty() {
        while parts.last() == Some(&"") {
            parts.pop();
        }
    }
    parts
}
pub fn package_name(path: &Path, environment: &Environment) -> Result<String> {
    let apk = Apk::open(path).map_err(|error| super::Error::Parse(error.to_string()))?;
    if let Some(bytes) = apk
        .file_if_present("resources.arsc")
        .map_err(|error| super::Error::Parse(error.to_string()))?
    {
        super::resources::Table::parse(&bytes)
            .map_err(|error| super::Error::Parse(error.to_string()))?;
    }
    let manifest = apk
        .manifest()
        .map_err(|error| super::Error::Parse(error.to_string()))?;
    package_name_manifest(&manifest, environment)
}
pub fn split_name(path: &Path, environment: &Environment) -> Result<Option<String>> {
    package_name(path, environment)?;
    let apk = Apk::open(path).map_err(|error| super::Error::Parse(error.to_string()))?;
    let manifest = apk
        .manifest()
        .map_err(|error| super::Error::Parse(error.to_string()))?;
    Ok(attr_value(&manifest, "", "split").filter(|name| !name.is_empty()))
}
fn package_name_manifest(manifest: &Element, environment: &Environment) -> Result<String> {
    let (package, _, _) = cluster::names(manifest)?;
    let mut uses_split = false;
    let mut property_names = None;
    let mut property_values = None;
    for child in &manifest.children {
        match child.name.as_str() {
            "application" => {
                let mut sdk_libraries = Vec::new();
                let mut static_libraries = Vec::new();
                for entry in &child.children {
                    let name = attr_value(entry, ANDROID, "name");
                    match entry.name.as_str() {
                        "uses-sdk-library" => {
                            let version = attr_value(entry, ANDROID, "versionMajor")
                                .and_then(|value| convert_int(&value))
                                .unwrap_or(-1);
                            if name
                                .as_deref()
                                .is_none_or(|value| value.chars().all(java_whitespace))
                                || version < 0
                            {
                                return fail("Bad uses-sdk-library declaration");
                            }
                            if sdk_libraries.contains(&name) {
                                return fail("Depending on multiple versions of SDK library");
                            }
                            sdk_libraries.push(name);
                        }
                        "uses-static-library" => {
                            if name
                                .as_deref()
                                .is_none_or(|value| value.chars().all(java_whitespace))
                                || integer(entry, "version", -1) < 0
                                || attr_value(entry, ANDROID, "certDigest").is_none()
                            {
                                return fail("Bad uses-static-library declaration");
                            }
                            if static_libraries.contains(&name) {
                                return fail("Depending on multiple versions of static library");
                            }
                            static_libraries.push(name);
                            let mut entries: Vec<_> = entry.children.iter().collect();
                            while let Some(cert) = entries.pop() {
                                if cert.name == "additional-certificate"
                                    && attr_value(cert, ANDROID, "certDigest")
                                        .is_none_or(|v| v.is_empty())
                                {
                                    return fail("Bad additional-certificate declaration");
                                }
                                entries.extend(cert.children.iter());
                            }
                        }
                        "sdk-library" => {
                            if name.is_none() || integer(entry, "versionMajor", -1) < 0 {
                                return fail("Bad sdk-library declaration");
                            }
                        }
                        "static-library" => {
                            if name.is_none() || integer(entry, "version", -1) < 0 {
                                return fail("Bad static-library declaration");
                            }
                        }
                        "library" => {
                            if name.is_none() {
                                return fail("Bad library declaration");
                            }
                        }
                        _ => {}
                    }
                }
            }
            "uses-split" if !uses_split => {
                if attr_value(child, ANDROID, "name").is_none() {
                    return fail("<uses-split> tag requires android:name");
                }
                uses_split = true;
            }
            "uses-sdk" => sdk(child, environment)?,
            "overlay" => {
                property_names = attr_value(child, ANDROID, "requiredSystemPropertyName");
                property_values = attr_value(child, ANDROID, "requiredSystemPropertyValue");
            }
            _ => {}
        }
    }
    let names = property_names.as_deref().unwrap_or("");
    let values = property_values.as_deref().unwrap_or("");
    if names.is_empty() != values.is_empty() {
        return fail("Overlay has incomplete required system property condition");
    }
    if !names.is_empty() {
        let names = java_split(names);
        let values = java_split(values);
        if names.len() != values.len()
            || names.iter().zip(values).any(|(name, value)| {
                environment
                    .properties
                    .get(*name)
                    .map(String::as_str)
                    .unwrap_or("")
                    != value
            })
        {
            return fail("Overlay ignored due to required system property");
        }
    }
    Ok(package)
}
fn convert_int(value: &str) -> Option<i32> {
    if value.is_empty() {
        return None;
    }
    let (negative, value) = value
        .strip_prefix('-')
        .map_or((false, value), |v| (true, v));
    let (radix, value) = if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        (16, value)
    } else if let Some(value) = value.strip_prefix('#') {
        (16, value)
    } else if value.len() > 1 && value.starts_with('0') {
        (8, &value[1..])
    } else {
        (10, value)
    };
    let parsed = i64::from_str_radix(value, radix).ok()?;
    i32::try_from(if negative { -parsed } else { parsed }).ok()
}

fn java_whitespace(value: char) -> bool {
    matches!(value, '\u{9}'..='\u{d}' | '\u{1c}'..='\u{20}' | '\u{1680}' | '\u{2000}'..='\u{2006}' | '\u{2008}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{205f}' | '\u{3000}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_apps::res::{Attr, Value};
    fn element(name: &str, attrs: &[(&str, &str, &str)], children: Vec<Element>) -> Element {
        Element {
            name: name.into(),
            attrs: attrs
                .iter()
                .map(|(ns, name, value)| Attr {
                    ns: (*ns).into(),
                    name: (*name).into(),
                    id: 0,
                    value: Value::String((*value).into()),
                    kind: 3,
                    data: 0,
                })
                .collect(),
            children,
        }
    }
    fn environment() -> Environment {
        Environment {
            sdk: 36,
            codenames: vec![],
            properties: BTreeMap::new(),
        }
    }
    #[test]
    fn lite_accepts_no_application_and_ignores_full_parser_only_rejections() {
        let manifest = element(
            "manifest",
            &[("", "package", "org.example.lite")],
            vec![
                element(
                    "uses-sdk",
                    &[(ANDROID, "targetSdkVersion", "36")],
                    vec![element("extension-sdk", &[], vec![])],
                ),
                element("application", &[], vec![element("activity", &[], vec![])]),
            ],
        );
        assert_eq!(
            package_name_manifest(&manifest, &environment()).unwrap(),
            "org.example.lite"
        );
        let manifest = element("manifest", &[("", "package", "org.example.lite")], vec![]);
        assert!(package_name_manifest(&manifest, &environment()).is_ok());
    }
    #[test]
    fn lite_checks_sdk_library_duplicates_and_live_overlay_properties() {
        let make = |children| element("manifest", &[("", "package", "org.example.lite")], children);
        assert!(
            package_name_manifest(
                &make(vec![element(
                    "uses-sdk",
                    &[(ANDROID, "minSdkVersion", "37")],
                    vec![]
                )]),
                &environment()
            )
            .is_err()
        );
        assert!(
            package_name_manifest(
                &make(vec![element(
                    "uses-sdk",
                    &[(ANDROID, "targetSdkVersion", "Preview")],
                    vec![]
                )]),
                &environment()
            )
            .is_err()
        );
        let library = || {
            element(
                "uses-sdk-library",
                &[(ANDROID, "name", "sdk"), (ANDROID, "versionMajor", "0x10")],
                vec![],
            )
        };
        assert!(
            package_name_manifest(
                &make(vec![element(
                    "application",
                    &[],
                    vec![library(), library()]
                )]),
                &environment()
            )
            .is_err()
        );
        let manifest = make(vec![element(
            "overlay",
            &[
                (ANDROID, "requiredSystemPropertyName", "first,second,"),
                (ANDROID, "requiredSystemPropertyValue", "a,b,"),
            ],
            vec![],
        )]);
        let mut env = environment();
        env.properties.insert("first".into(), "a".into());
        env.properties.insert("second".into(), "b".into());
        assert!(package_name_manifest(&manifest, &env).is_ok());
        env.properties.insert("second".into(), "no".into());
        assert!(package_name_manifest(&manifest, &env).is_err());
        assert!(!java_whitespace('\u{a0}'));
        assert!(!java_whitespace('\u{2007}'));
        assert!(java_whitespace('\u{2008}'));
    }
}
