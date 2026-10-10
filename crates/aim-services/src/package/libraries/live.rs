//! Shared library provider refresh during a live native install (#986).
//! Retains registered info identity/order while its code owner changes.
use super::{NotModelled, Registry, TYPE_DYNAMIC, TYPE_SDK_PACKAGE, TYPE_STATIC};
impl Registry {
    pub(in crate::package) fn check_live_provider(
        &self,
        record: &crate::package::scan::Record,
    ) -> Result<(), NotModelled> {
        let pkg = &record.parsed;
        let provider = if let Some(name) = &pkg.sdk_library_name {
            Some((name, pkg.sdk_lib_version_major as i64, TYPE_SDK_PACKAGE))
        } else if let Some(name) = &pkg.static_shared_library_name {
            Some((name, pkg.static_shared_lib_version, TYPE_STATIC))
        } else {
            None
        };
        if let Some((name, version, kind)) = provider {
            if let Some(existing) = self.get(name, version) {
                if existing.kind != kind
                    || existing.package_name.as_deref() != Some(record.settings.name.as_str())
                {
                    return Err(NotModelled(
                        "a live SDK/static library version is owned by another package",
                    ));
                }
            }
        }
        Ok(())
    }
    pub(in crate::package) fn refresh_live_provider(
        &mut self,
        record: &crate::package::scan::Record,
    ) -> Result<(), NotModelled> {
        let mut paths = vec![Some(
            record
                .parsed
                .base_apk_path
                .clone()
                .ok_or(NotModelled("live provider has no base APK"))?,
        )];
        paths.extend(record.parsed.split_code_paths.clone().unwrap_or_default());
        if paths.iter().any(Option::is_none) {
            return Err(NotModelled("live provider has a null split APK"));
        }
        let version = (record.parsed.version_code_major as i64) << 32
            | record.parsed.version_code as u32 as i64;
        for entry in self
            .entries
            .values_mut()
            .flat_map(|versions| versions.values_mut())
        {
            if entry.package_name.as_deref() == Some(record.settings.name.as_str())
                && matches!(entry.kind, TYPE_DYNAMIC | TYPE_SDK_PACKAGE | TYPE_STATIC)
            {
                entry.code_paths = Some(paths.clone());
                entry.declaring.1 = version;
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{
        libraries::{SharedLibrary, VERSION_UNDEFINED},
        owner::shared_users::ScanOrigin,
        pkg::AndroidPackage,
        scan::{Identity, Record},
        settings,
        sign::SigningDetails,
    };
    #[test]
    fn refresh_retains_equal_hash_provider_order_and_other_owner_paths() {
        let mut registry = Registry::default();
        for name in ["BB", "Aa"] {
            registry.insert(SharedLibrary {
                name: Some(name.into()),
                package_name: Some("provider".into()),
                code_paths: Some(vec![Some("old.apk".into())]),
                version: VERSION_UNDEFINED,
                kind: TYPE_DYNAMIC,
                ..Default::default()
            });
        }
        registry.insert(SharedLibrary {
            name: Some("other".into()),
            package_name: Some("different".into()),
            code_paths: Some(vec![Some("different.apk".into())]),
            version: VERSION_UNDEFINED,
            kind: TYPE_DYNAMIC,
            ..Default::default()
        });
        let record = Record {
            settings: settings::Package {
                name: "provider".into(),
                ..Default::default()
            },
            parsed: AndroidPackage {
                package_name: "provider".into(),
                base_apk_path: Some("new.apk".into()),
                version_code: 8,
                ..Default::default()
            },
            signing: SigningDetails::unknown(),
            identity: Identity {
                manifest_name: "provider".into(),
                internal_name: "provider".into(),
                real_name: None,
            },
            origin: ScanOrigin::Data,
        };
        registry.refresh_live_provider(&record).unwrap();
        let collision: Vec<_> = registry
            .entries()
            .filter_map(|entry| entry.name.as_deref())
            .filter(|name| matches!(*name, "BB" | "Aa"))
            .collect();
        assert_eq!(collision, ["BB", "Aa"]);
        assert_eq!(
            registry
                .get("BB", VERSION_UNDEFINED)
                .unwrap()
                .code_paths
                .as_ref()
                .unwrap(),
            &[Some("new.apk".into())]
        );
        assert_eq!(
            registry
                .get("other", VERSION_UNDEFINED)
                .unwrap()
                .code_paths
                .as_ref()
                .unwrap(),
            &[Some("different.apk".into())]
        );
    }
}
