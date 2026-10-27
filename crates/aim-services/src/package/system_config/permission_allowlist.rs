//! SystemConfig.readPermissionAllowlist, android-16.0.0_r1.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PermissionAllowlist {
    /// Partition/APEX, declaration kind, package, permission; explicit denies retained.
    entries: BTreeMap<(String, String, String, String), bool>,
}
impl PermissionAllowlist {
    pub(super) fn read(&mut self, element: &Element, image: &Path, file: &Path) {
        let Some(package) = element.string("package").filter(|p| !p.is_empty()) else {
            return;
        };
        let Ok(relative) = file.strip_prefix(image) else {
            return;
        };
        let mut components = relative.iter();
        let partition = components.next().unwrap_or_default().to_string_lossy();
        let namespace = match partition.as_ref() {
            "odm" => "vendor".to_owned(),
            "apex" if element.name == "signature-permissions" => "apex".to_owned(),
            "apex" => format!(
                "apex/{}",
                components.next().unwrap_or_default().to_string_lossy()
            ),
            _ => partition.into_owned(),
        };
        for child in element.children() {
            let allowed = match child.name.as_str() {
                "permission" => true,
                "deny-permission" => false,
                _ => continue,
            };
            let Some(name) = child.string("name").filter(|n| !n.is_empty()) else {
                continue;
            };
            self.entries.insert(
                (
                    namespace.clone(),
                    element.name.clone(),
                    package.to_string(),
                    name.into_owned(),
                ),
                allowed,
            );
        }
    }
    pub fn get(
        &self,
        namespace: &str,
        kind: &str,
        package: &str,
        permission: &str,
    ) -> Option<bool> {
        self.entries
            .get(&(
                namespace.into(),
                kind.into(),
                package.into(),
                permission.into(),
            ))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partition_and_apex_scopes_preserve_explicit_denies() {
        let image = Path::new("image");
        let mut list = PermissionAllowlist::default();
        for (file, xml) in [
            (
                "vendor/etc/permissions/a.xml",
                "<privapp-permissions package='p'><permission name='allowed'/><deny-permission name='denied'/></privapp-permissions>",
            ),
            (
                "odm/etc/permissions/b.xml",
                "<signature-permissions package='p'><permission name='signature'/></signature-permissions>",
            ),
            (
                "apex/module/etc/permissions/a.xml",
                "<privapp-permissions package='p'><permission name='apex'/></privapp-permissions>",
            ),
            (
                "apex/module/etc/permissions/a.xml",
                "<signature-permissions package='p'><permission name='apex'/></signature-permissions>",
            ),
        ] {
            list.read(
                &aim_android_xml::read(xml.as_bytes()).unwrap(),
                image,
                &image.join(file),
            );
        }
        assert_eq!(
            list.get("vendor", "privapp-permissions", "p", "allowed"),
            Some(true)
        );
        assert_eq!(
            list.get("vendor", "privapp-permissions", "p", "denied"),
            Some(false)
        );
        assert_eq!(
            list.get("system", "privapp-permissions", "p", "allowed"),
            None
        );
        assert_eq!(
            list.get("vendor", "signature-permissions", "p", "signature"),
            Some(true)
        );
        assert_eq!(
            list.get("apex/module", "privapp-permissions", "p", "apex"),
            Some(true)
        );
        assert_eq!(
            list.get("apex", "signature-permissions", "p", "apex"),
            Some(true)
        );
    }
}
