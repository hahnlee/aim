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
//!   text XML): the legacy runtime permission state, of which Android 16
//!   writes only the version, the default grants' fingerprint and the
//!   shared users.

use aim_android_xml::Element;

use super::{children, string};

/// `RuntimePermissionsState`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RuntimePermissions {
    /// `NO_VERSION` (-1) when the file has none.
    pub version: i32,
    pub fingerprint: Option<String>,
    pub packages: Vec<(String, Vec<RuntimePermission>)>,
    pub shared_users: Vec<(String, Vec<RuntimePermission>)>,
}

/// `RuntimePermissionsState.PermissionState`: `flags` are
/// `PackageManager.FLAG_PERMISSION_*`.
#[derive(Clone, Debug, PartialEq)]
pub struct RuntimePermission {
    pub name: String,
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
                        name: string(p, "name").ok_or("a permission without a name")?,
                        // `Boolean.parseBoolean`: anything else is false.
                        granted: string(p, "granted")
                            .is_some_and(|g| g.eq_ignore_ascii_case("true")),
                        flags: p.int_hex("flags")?.ok_or("a permission without flags")?,
                    })
                })
                .collect::<Result<_, String>>()?;
            list.push((string(e, "name").unwrap_or_default(), permissions));
        }
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
                list.retain(|q| q.name != permission.name);
                list.push(permission);
            }
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
