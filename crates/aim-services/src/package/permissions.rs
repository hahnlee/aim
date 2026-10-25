//! The permission state PackageManager's permission service keeps
//! (docs/permissions.md, "Persistence"):
//!
//! - `access.abx`, `AccessCheckingService`'s state (`AccessPersistence`,
//!   always binary XML): the permission definitions and trees in
//!   `/data/misc/apexdata/com.android.permission`; per user in
//!   `/data/misc_de/<user>/apexdata/com.android.permission`, each app id's
//!   permissions with their flags (`PermissionFlags`, granted is a flag),
//!   per device too, the app ops' modes by app id and by package, the
//!   package versions the state was upgraded for and the default grants'
//!   fingerprint;
//! - `runtime-permissions.xml` beside the user's `access.abx`
//!   (`RuntimePermissionsPersistenceImpl` in the permission module, always
//!   text XML): the legacy runtime permission state, with version, the
//!   default grants' fingerprint and package/shared permission lists. The
//!   writer clears one-time grants while retaining their flags.

use aim_android_xml::Element;

mod runtime_write;

use super::{children, string};

/// `RuntimePermissionsState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimePermissions {
    /// `NO_VERSION` (-1) when the file has none.
    pub version: i32,
    pub fingerprint: Option<String>,
    /// Original ArrayMap keys may be null, distinct from the empty string.
    pub packages: Vec<(Option<String>, Vec<RuntimePermission>)>,
    pub shared_users: Vec<(Option<String>, Vec<RuntimePermission>)>,
}

/// `RuntimePermissionsState.PermissionState`: `flags` are
/// `PackageManager.FLAG_PERMISSION_*`.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimePermission {
    pub name: Option<String>,
    pub granted: bool,
    pub flags: i32,
}

impl RuntimePermissions {
    /// `parseRuntimePermissions`, on the document whose root is `root`.
    pub fn parse(root: &Element) -> Result<RuntimePermissions, String> {
        if root.name != "runtime-permissions" {
            return Err(format!("<{}>, not <runtime-permissions>", root.name));
        }
        let mut r = RuntimePermissions {
            version: root.int("version")?.unwrap_or(-1),
            fingerprint: string(root, "fingerprint"),
            ..RuntimePermissions::default()
        };
        for e in root.children() {
            let list = match e.name.as_str() {
                "package" => &mut r.packages,
                "shared-user" => &mut r.shared_users,
                _ => continue,
            };
            let permissions = children(e, "permission")
                .map(|p| {
                    Ok(RuntimePermission {
                        name: string(p, "name"),
                        // `Boolean.parseBoolean`: anything else is false.
                        granted: string(p, "granted")
                            .is_some_and(|g| g.eq_ignore_ascii_case("true")),
                        flags: p.int_hex("flags")?.ok_or("a permission without flags")?,
                    })
                })
                .collect::<Result<_, String>>()?;
            let name = string(e, "name");
            if let Some((_, value)) = list.iter_mut().find(|(key, _)| *key == name) {
                *value = permissions;
            } else {
                list.push((name, permissions));
            }
        }
        r.packages
            .sort_by_key(|(name, _)| name.as_deref().map(super::info::java_hash).unwrap_or(0));
        r.shared_users
            .sort_by_key(|(name, _)| name.as_deref().map(super::info::java_hash).unwrap_or(0));
        Ok(r)
    }
}

/// The system part of `access.abx`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccessSystem {
    pub permission_trees: Vec<Permission>,
    pub permissions: Vec<Permission>,
}

/// `Permission` (`services/permission`): a definition.
#[derive(Clone, Debug, PartialEq)]
pub struct Permission {
    pub name: String,
    pub package: String,
    pub protection_level: i32,
    /// A dynamic permission's icon and label; `None` for a manifest one.
    pub dynamic: Option<(i32, Option<String>)>,
}

/// `Permission.TYPE_MANIFEST`, `TYPE_DYNAMIC`.
const TYPE_MANIFEST: i32 = 0;
const TYPE_DYNAMIC: i32 = 2;

/// Permission names with their flags, or app op names with their modes.
pub type Named = Vec<(String, i32)>;

/// A user's part of `access.abx`. Lists keep the file's order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccessUser {
    /// The package versions the state was upgraded for.
    pub package_versions: Vec<(String, i32)>,
    pub default_permission_grant_fingerprint: Option<String>,
    /// By app id: permission names and their `PermissionFlags`.
    pub app_id_permissions: Vec<(i32, Named)>,
    /// By app id and virtual device id: permission names and flags.
    pub app_id_device_permissions: Vec<(i32, Vec<(String, Named)>)>,
    /// By app id: app op names and modes.
    pub app_id_app_ops: Vec<(i32, Named)>,
    /// By package: app op names and modes.
    pub package_app_ops: Vec<(String, Named)>,
}

/// `getAttributeValueOrThrow`.
fn required_string(e: &Element, name: &str) -> Result<String, String> {
    string(e, name).ok_or_else(|| format!("<{}> without {name}", e.name))
}

/// `getAttributeIntOrThrow`.
fn required_int(e: &Element, name: &str) -> Result<i32, String> {
    e.int(name)?
        .ok_or_else(|| format!("<{}> without {name}", e.name))
}

/// The `<access>` element of `root`'s document, as `AccessPolicy` finds it.
fn access(root: &Element) -> Result<&Element, String> {
    if root.name == "access" {
        Ok(root)
    } else {
        Err(format!("<{}>, not <access>", root.name))
    }
}

/// `name`-flag pairs: the `<permission>`s or `<app-op>`s under `e`.
fn named_ints(e: &Element, tag: &str, value: &str) -> Result<Named, String> {
    children(e, tag)
        .map(|c| Ok((required_string(c, "name")?, required_int(c, value)?)))
        .collect()
}

/// The `<app-id>`s under `e`, each with what `each` reads of it.
fn by_app_id<T>(
    e: &Element,
    each: impl Fn(&Element) -> Result<T, String>,
) -> Result<Vec<(i32, T)>, String> {
    children(e, "app-id")
        .map(|a| Ok((required_int(a, "id")?, each(a)?)))
        .collect()
}

impl AccessSystem {
    /// `AccessPolicy.parseSystemState`: definitions of an unknown type are
    /// left out, as the original leaves them out.
    pub fn parse(root: &Element) -> Result<AccessSystem, String> {
        let mut s = AccessSystem::default();
        for e in access(root)?.children() {
            let list = match e.name.as_str() {
                "permissions" => &mut s.permissions,
                "permission-trees" => &mut s.permission_trees,
                _ => continue,
            };
            for p in children(e, "permission") {
                let dynamic = match required_int(p, "type")? {
                    TYPE_MANIFEST => None,
                    TYPE_DYNAMIC => Some((p.int_hex("icon")?.unwrap_or(0), string(p, "label"))),
                    _ => continue,
                };
                let permission = Permission {
                    name: required_string(p, "name")?,
                    package: required_string(p, "packageName")?,
                    protection_level: p
                        .int_hex("protectionLevel")?
                        .ok_or("a permission without a protection level")?,
                    dynamic,
                };
                if let Some(old) = list.iter_mut().find(|q| q.name == permission.name) {
                    *old = permission;
                } else {
                    list.push(permission);
                }
            }
        }
        // `MutableIndexedMap` is an ArrayMap: stable within a hash collision.
        for list in [&mut s.permissions, &mut s.permission_trees] {
            list.sort_by_key(|p| super::info::java_hash(&p.name));
        }
        Ok(s)
    }
}

impl AccessUser {
    /// `AccessPolicy.parseUserState`.
    pub fn parse(root: &Element) -> Result<AccessUser, String> {
        let mut u = AccessUser::default();
        for e in access(root)?.children() {
            match e.name.as_str() {
                "package-versions" => {
                    u.package_versions = named_ints(e, "package", "version")?;
                }
                "default-permission-grant" => {
                    u.default_permission_grant_fingerprint =
                        Some(required_string(e, "fingerprint")?);
                }
                "app-id-permissions" => {
                    u.app_id_permissions = by_app_id(e, |a| named_ints(a, "permission", "flags"))?;
                }
                "app-id-device-permissions" => {
                    u.app_id_device_permissions = by_app_id(e, |a| {
                        children(a, "device")
                            .map(|d| {
                                Ok((
                                    required_string(d, "id")?,
                                    named_ints(d, "permission", "flags")?,
                                ))
                            })
                            .collect()
                    })?;
                }
                "app-id-app-ops" => {
                    u.app_id_app_ops = by_app_id(e, |a| named_ints(a, "app-op", "mode"))?;
                }
                "package-app-ops" => {
                    u.package_app_ops = children(e, "package")
                        .map(|p| {
                            Ok((
                                required_string(p, "name")?,
                                named_ints(p, "app-op", "mode")?,
                            ))
                        })
                        .collect::<Result<_, String>>()?;
                }
                _ => {}
            }
        }
        Ok(u)
    }
}

/// `PermissionFlags.ONE_TIME`, `RUNTIME_GRANTED`.
const ONE_TIME: i32 = 1 << 21;
const RUNTIME_GRANTED: i32 = 1 << 4;

impl AccessUser {
    /// `AppIdPermissionPolicy.getPermissionFlags`: 0 for a permission or
    /// app id without state.
    pub fn permission_flags(&self, app_id: i32, permission: &str) -> i32 {
        self.app_id_permissions
            .iter()
            .find(|(id, _)| *id == app_id)
            .and_then(|(_, permissions)| permissions.iter().find(|(name, _)| name == permission))
            .map_or(0, |(_, flags)| *flags)
    }

    /// `AppIdPermissionPolicy.setPermissionFlags`: the app id's flags of
    /// `permission` replaced, a permission left without flags removed and
    /// an app id left without permissions too (`putWithDefault`), the
    /// app ids in their `SparseArray` order and each one's permissions in
    /// their `ArrayMap` order. The flags before.
    pub fn set_permission_flags(&mut self, app_id: i32, permission: &str, flags: i32) -> i32 {
        let at = match self
            .app_id_permissions
            .binary_search_by_key(&app_id, |(id, _)| *id)
        {
            Ok(at) => at,
            Err(_) if flags == 0 => return 0,
            Err(at) => {
                self.app_id_permissions.insert(at, (app_id, Vec::new()));
                at
            }
        };
        let permissions = &mut self.app_id_permissions[at].1;
        let old = match permissions.iter().position(|(name, _)| name == permission) {
            Some(i) if flags == 0 => permissions.remove(i).1,
            Some(i) => std::mem::replace(&mut permissions[i].1, flags),
            None if flags == 0 => 0,
            None => {
                // `ArrayMap.put`: after the keys of its hash and below.
                let hash = super::info::java_hash(permission);
                let i = permissions
                    .iter()
                    .position(|(name, _)| super::info::java_hash(name) > hash)
                    .unwrap_or(permissions.len());
                permissions.insert(i, (permission.to_owned(), flags));
                0
            }
        };
        if self.app_id_permissions[at].1.is_empty() {
            self.app_id_permissions.remove(at);
        }
        old
    }
}

/// The user state document `root` (`access.abx`) with its
/// `<app-id-permissions>` written as `AppIdPermissionPersistence
/// .serializeAppIdPermissions` writes `app_ids`: one-time permissions
/// never as granted. The other policies' parts are kept as read.
pub fn with_app_id_permissions(
    mut root: Element,
    app_ids: &[(i32, Named)],
) -> Result<Element, String> {
    use aim_android_xml::{Node, Value};
    access(&root)?;
    let element = |name: &str, attrs: Vec<(String, Value)>, content: Vec<Node>| {
        Node::Element(Element {
            name: name.into(),
            attrs,
            content,
        })
    };
    let written = element(
        "app-id-permissions",
        Vec::new(),
        app_ids
            .iter()
            .map(|(app_id, permissions)| {
                element(
                    "app-id",
                    vec![("id".into(), Value::Int(*app_id))],
                    permissions
                        .iter()
                        .map(|(name, flags)| {
                            let flags = if flags & ONE_TIME != 0 {
                                flags & !RUNTIME_GRANTED
                            } else {
                                *flags
                            };
                            element(
                                "permission",
                                vec![
                                    ("name".into(), Value::Interned(name.clone())),
                                    ("flags".into(), Value::Int(flags)),
                                ],
                                Vec::new(),
                            )
                        })
                        .collect(),
                )
            })
            .collect(),
    );
    match root
        .content
        .iter_mut()
        .find(|n| matches!(n, Node::Element(e) if e.name == "app-id-permissions"))
    {
        Some(node) => *node = written,
        None => return Err("<access> without <app-id-permissions>".into()),
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAMERA: &str = "android.permission.CAMERA";
    const POST_NOTIFICATIONS: &str = "android.permission.POST_NOTIFICATIONS";

    #[test]
    fn flags_are_set_in_their_maps_order() {
        let mut u = AccessUser::default();
        assert_eq!(u.set_permission_flags(10_100, CAMERA, 0), 0);
        assert!(u.app_id_permissions.is_empty());
        assert_eq!(
            u.set_permission_flags(10_100, POST_NOTIFICATIONS, 1 << 4),
            0
        );
        assert_eq!(u.set_permission_flags(10_100, CAMERA, 1 << 5), 0);
        assert_eq!(u.set_permission_flags(10_050, CAMERA, 1 << 5), 0);
        let names: Vec<&str> = u.app_id_permissions[1]
            .1
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        let mut by_hash = vec![CAMERA, POST_NOTIFICATIONS];
        by_hash.sort_by_key(|n| super::super::info::java_hash(n));
        assert_eq!(names, by_hash);
        assert_eq!(u.app_id_permissions[0].0, 10_050);
        assert_eq!(
            u.set_permission_flags(10_100, POST_NOTIFICATIONS, 1 << 5),
            1 << 4
        );
        assert_eq!(u.permission_flags(10_100, POST_NOTIFICATIONS), 1 << 5);
        u.set_permission_flags(10_050, CAMERA, 0);
        assert_eq!(u.app_id_permissions.len(), 1);
    }

    /// The written app id permissions read back as the model, one-time
    /// grants not persisted, the other parts kept.
    #[test]
    fn app_id_permissions_are_written_back() {
        let root = aim_android_xml::text::read(
            br#"<access><package-versions><package name="p" version="3"/></package-versions><app-id-permissions><app-id id="1"><permission name="x" flags="1"/></app-id></app-id-permissions></access>"#,
        )
        .unwrap();
        let mut u = AccessUser::parse(&root).unwrap();
        u.set_permission_flags(10_100, CAMERA, RUNTIME_GRANTED | ONE_TIME);
        let written = with_app_id_permissions(root, &u.app_id_permissions).unwrap();
        let bytes = aim_android_xml::abx::write(&written).unwrap();
        let read = AccessUser::parse(&aim_android_xml::read(&bytes).unwrap()).unwrap();
        assert_eq!(read.package_versions, [("p".to_string(), 3)]);
        assert_eq!(read.permission_flags(1, "x"), 1);
        assert_eq!(read.permission_flags(10_100, CAMERA), ONE_TIME);
    }
}
