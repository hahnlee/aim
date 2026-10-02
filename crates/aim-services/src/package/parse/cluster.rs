//! APK cluster selection and split parsing, ported from AOSP
//! android-16.0.0_r1 `ApkLiteParseUtils` and `ParsingPackageUtils`,
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::collections::BTreeMap;
use std::path::Path;

use super::*;

pub(super) struct Part {
    pub path: String,
    pub manifest: Element,
    pub table: Option<Table>,
    pub split: Option<String>,
    pub revision: i32,
}

pub(super) fn load(host: &Path, path: &str) -> Result<Vec<Part>> {
    let files = if host.is_dir() {
        std::fs::read_dir(host)
            .map_err(|e| Error::Parse(e.to_string()))?
            .map(|entry| {
                entry
                    .map(|e| e.path())
                    .map_err(|e| Error::Parse(e.to_string()))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(".apk"))
            })
            .map(|p| {
                let guest = format!("{path}/{}", p.file_name().unwrap().to_string_lossy());
                (p, guest)
            })
            .collect::<Vec<_>>()
    } else {
        vec![(host.to_owned(), path.to_owned())]
    };
    if files.is_empty() {
        return fail("No packages found in split");
    }
    let mut parts = BTreeMap::new();
    let mut identity = None;
    for (host, path) in files {
        let apk = Apk::open(&host).map_err(|e| Error::Parse(e.to_string()))?;
        let manifest = apk.manifest().map_err(|e| Error::Parse(e.to_string()))?;
        let (package, split, version) = names(&manifest)?;
        if identity
            .as_ref()
            .is_some_and(|(name, v)| name != &package || *v != version)
        {
            return fail(format!("Inconsistent package or version in {path}"));
        }
        identity = Some((package, version));
        let table = apk
            .file("resources.arsc")
            .ok()
            .map(|bytes| Table::parse(&bytes))
            .transpose()
            .map_err(|e| Error::Parse(e.to_string()))?;
        let revision = integer(&manifest, "revisionCode");
        let part = Part {
            path,
            manifest,
            table,
            split: split.clone(),
            revision,
        };
        if parts.insert(split, part).is_some() {
            return fail("Split name defined more than once");
        }
    }
    let base = parts
        .remove(&None)
        .ok_or_else(|| Error::Parse("Missing base APK".into()))?;
    if !parts.is_empty() && attr_bool(&base.manifest, ANDROID, "isolatedSplits", false) {
        return Err(Error::Unsupported(
            "isolated split asset dependencies (#720)".into(),
        ));
    }
    Ok(std::iter::once(base).chain(parts.into_values()).collect())
}

fn names(manifest: &Element) -> Result<(String, Option<String>, i32)> {
    if manifest.name != "manifest" {
        return fail("No <manifest> tag");
    }
    for name in ["requiredSplitTypes", "splitTypes"] {
        if let Some(value) = attr_value(manifest, ANDROID, name).filter(|s| !s.is_empty()) {
            validate_split_types(&value)?;
        }
    }
    let package = attr_value(manifest, "", "package").unwrap_or_default();
    if package.is_empty() {
        return fail("<manifest> has no package");
    }
    if package != "android"
        && let Some(e) = validate_filename_name(&package, true)
    {
        return fail(format!("Invalid manifest package: {e}"));
    }
    let split = attr_value(manifest, "", "split").filter(|s| !s.is_empty());
    if let Some(split) = &split
        && let Some(e) = validate_name(split, false)
    {
        return fail(format!("Invalid manifest split: {e}"));
    }
    Ok((package, split, integer(manifest, "versionCode")))
}

// ApkLite keeps the sets for install validation, not in the parser cache.
fn validate_split_types(value: &str) -> Result<()> {
    let value = value.trim_matches(|c| c <= '\u{20}');
    let mut types: Vec<_> = value.split(',').collect();
    if value.contains(',') {
        while types.last() == Some(&"") {
            types.pop();
        }
    }
    for name in types {
        let name = name.trim_matches(|c| c <= '\u{20}');
        if let Some(error) = validate_filename_name(name, false) {
            return fail(format!("Invalid manifest split types: {error}"));
        }
    }
    Ok(())
}

fn validate_filename_name(name: &str, separator: bool) -> Option<String> {
    validate_name(name, separator).or_else(|| {
        if matches!(name, "" | "." | "..") {
            Some("Invalid filename".into())
        } else if name.len() > 223 {
            Some("the length of the name is greater than 223".into())
        } else {
            None
        }
    })
}

fn integer(manifest: &Element, name: &str) -> i32 {
    manifest
        .attrs
        .iter()
        .find(|a| a.ns == ANDROID && a.name == name)
        .filter(|a| (TYPE_FIRST_INT..=TYPE_LAST_INT).contains(&a.kind))
        .map_or(0, |a| a.data as i32)
}

impl Parser<'_> {
    pub(super) fn parse_split(&self, pkg: &mut Package, part: &Part, index: usize) -> Result<()> {
        let mut found = false;
        for e in &part.manifest.children {
            if self.skip(pkg, e) || e.name != "application" || found {
                continue;
            }
            found = true;
            let sa = self.obtain(e);
            pkg.split_flags.as_mut().unwrap()[index] =
                if sa.boolean("hasCode", true) { 4 } else { 0 };
            let loader = sa.string("classLoader");
            if loader.as_ref().is_some_and(|s| {
                s != "dalvik.system.PathClassLoader"
                    && s != "dalvik.system.DexClassLoader"
                    && s != "dalvik.system.DelegateLastClassLoader"
            }) {
                return fail(format!("Invalid class loader name: {loader:?}"));
            }
            pkg.split_class_loader_names.as_mut().unwrap()[index] = loader;
            let split = part.split.as_deref();
            for c in &e.children {
                if self.skip(pkg, c) {
                    continue;
                }
                match c.name.as_str() {
                    "activity" | "receiver" => {
                        let a = self.parse_activity_or_receiver(pkg, c, split)?;
                        pkg.add_mime_groups(&a.main.component.intents);
                        if c.name == "activity" {
                            pkg.activities.push(a)
                        } else {
                            pkg.receivers.push(a)
                        }
                    }
                    "service" => {
                        let s = self.parse_service(pkg, c, split)?;
                        pkg.add_mime_groups(&s.main.component.intents);
                        pkg.services.push(s);
                    }
                    "provider" => {
                        let p = self.parse_provider(pkg, c, split)?;
                        pkg.add_mime_groups(&p.main.component.intents);
                        pkg.providers.push(p);
                    }
                    "activity-alias" => {
                        let a = self.parse_activity_alias(pkg, c, split)?;
                        pkg.add_mime_groups(&a.main.component.intents);
                        pkg.activities.push(a);
                    }
                    "meta-data"
                    | "property"
                    | "uses-sdk-library"
                    | "uses-static-library"
                    | "uses-library"
                    | "uses-native-library"
                    | "uses-package" => self.parse_base_app_child_tag(pkg, c)?,
                    _ => {}
                }
            }
        }
        if !found {
            self.input.borrow_mut().defer(
                "<manifest> does not contain an <application>".into(),
                MISSING_APP_TAG,
            )?;
        }
        Ok(())
    }
}
