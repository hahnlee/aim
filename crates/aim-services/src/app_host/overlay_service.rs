//! `overlay`: OverlayManagerService for the app host (`android-16.0.0_r1`):
//! the overlays of a target, as `getOverlayInfosForTarget` reads them from
//! the overlay settings the original last persisted
//! (`/data/system/overlays.xml`, `OverlayManagerSettings.restore`), in
//! their priority order. The settings are not reconciled with the
//! installed packages, and nothing changes them; the other methods are not
//! implemented (#1640).
//! The original service's constructor needs UserManagerService's singleton
//! and local package/user owners, absent from the app host's system process.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use std::sync::{Arc, OnceLock};

use aim_binder_host::local::{Call, Reply, Service};
use aim_binder_host::parcel::{BAD_VALUE, EX_ILLEGAL_STATE, Exception, Parcel};
use aim_service_aidl::WriteParcelable;
use aim_service_aidl::android_content_om_ioverlaymanager as om;

use super::activity_user;
use super::{AppHost, reply};

/// `OverlayManagerSettings.Serializer.CURRENT_VERSION`; versions 3 and 4
/// read as it.
const CURRENT_VERSION: i32 = 5;

/// `OverlayInfo` as `SettingsItem.getOverlayInfo` makes it.
#[derive(Clone, Debug, PartialEq)]
struct OverlayInfo {
    package_name: Option<String>,
    overlay_name: Option<String>,
    target_package_name: Option<String>,
    target_overlayable_name: Option<String>,
    category: Option<String>,
    base_code_path: Option<String>,
    state: i32,
    user_id: i32,
    priority: i32,
    is_mutable: bool,
    is_fabricated: bool,
    /// `OverlayConstraint`s: type and value.
    constraints: Vec<(i32, i32)>,
}

impl WriteParcelable for OverlayInfo {
    fn write_to(&self, p: &mut Parcel) {
        for s in [
            &self.package_name,
            &self.overlay_name,
            &self.target_package_name,
            &self.target_overlayable_name,
            &self.category,
            &self.base_code_path,
        ] {
            p.write_string16(s.as_deref());
        }
        p.write_i32(self.state);
        p.write_i32(self.user_id);
        p.write_i32(self.priority);
        p.write_bool(self.is_mutable);
        p.write_bool(self.is_fabricated);
        // `writeTypedArray`: its length, then each present constraint.
        p.write_i32(self.constraints.len() as i32);
        for (kind, value) in &self.constraints {
            p.write_i32(1);
            p.write_i32(*kind);
            p.write_i32(*value);
        }
    }
}

/// `Serializer.restore`: the settings' items, in their order.
fn restore(bytes: &[u8]) -> Result<Vec<OverlayInfo>, String> {
    let root = aim_android_xml::read(bytes)?;
    if root.name != "overlays" {
        return Err(format!("overlays.xml: <{}>", root.name));
    }
    match root.int("version")? {
        Some(3..=CURRENT_VERSION) => {}
        version => return Err(format!("overlays.xml: version {version:?}")),
    }
    let mut items = Vec::new();
    for item in root.children().filter(|e| e.name == "item") {
        let string = |name: &str| item.string(name).map(|s| s.into_owned());
        let int = |name: &str| {
            item.int(name)
                .and_then(|v| v.ok_or_else(|| format!("overlays.xml: no {name}")))
        };
        let constraints = item
            .children()
            .filter(|e| e.name == "constraint")
            .map(|c| Ok((c.int("type")?.unwrap_or(0), c.int("value")?.unwrap_or(0))))
            .collect::<Result<_, String>>()?;
        items.push(OverlayInfo {
            package_name: string("packageName"),
            overlay_name: string("overlayName"),
            target_package_name: string("targetPackageName"),
            target_overlayable_name: string("targetOverlayableName"),
            category: string("category"),
            base_code_path: string("baseCodePath"),
            state: int("state")?,
            user_id: int("userId")?,
            priority: int("priority")?,
            is_mutable: !item.bool("isStatic")?.unwrap_or(false),
            is_fabricated: item.bool("fabricated")?.unwrap_or(false),
            constraints,
        });
    }
    Ok(items)
}

pub struct OverlayManager {
    host: Arc<AppHost>,
    settings: OnceLock<Result<Vec<OverlayInfo>, String>>,
}

impl OverlayManager {
    pub fn new(host: Arc<AppHost>) -> Self {
        OverlayManager {
            host,
            settings: OnceLock::new(),
        }
    }

    fn settings(&self) -> Result<&[OverlayInfo], Exception> {
        let data = self.host.data()?;
        let settings = self.settings.get_or_init(|| {
            let path = data.join("system/overlays.xml");
            match std::fs::read(&path) {
                Ok(bytes) => restore(&bytes),
                // OverlayManagerService.restoreSettingsLocked leaves the
                // initial settings empty until the first persisted change.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                Err(e) => Err(format!("/data/system/overlays.xml: {e}")),
            }
        });
        settings
            .as_deref()
            .map_err(|e| Exception::new(EX_ILLEGAL_STATE, e.clone()))
    }
}

impl Service for OverlayManager {
    fn descriptor(&self) -> &str {
        om::DESCRIPTOR
    }

    fn transact(&self, call: &mut Call<'_>) -> Reply {
        let mut p = Parcel::new();
        match call.code {
            om::GET_OVERLAY_INFOS_FOR_TARGET => {
                let a =
                    om::GetOverlayInfosForTarget::read(&mut call.data).map_err(|_| BAD_VALUE)?;
                let Some(target) = a.target_package_name else {
                    om::write_get_overlay_infos_for_target_reply::<OverlayInfo>(&mut p, Some(&[]));
                    return Ok(p);
                };
                let user = self.host.handle_incoming_user(
                    call.sender_euid as i32,
                    a.user_id,
                    false,
                    activity_user::AllowMode::FullOnly,
                    Some("getOverlayInfosForTarget"),
                    None,
                );
                let overlays = user.and_then(|user| {
                    Ok(self
                        .settings()?
                        .iter()
                        .filter(|o| {
                            o.user_id == user
                                && o.target_package_name.as_deref() == Some(target.as_str())
                        })
                        .cloned()
                        .map(Some)
                        .collect::<Vec<_>>())
                });
                match overlays {
                    Ok(overlays) => {
                        om::write_get_overlay_infos_for_target_reply(&mut p, Some(&overlays))
                    }
                    Err(e) => return reply(Err(e)),
                }
            }
            code => {
                return reply(Err(self.host.unimplemented(
                    "overlay",
                    self.descriptor(),
                    code,
                )));
            }
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(
        process: &aim_binder_host::local::LocalService,
        target: Option<&str>,
        user: i32,
    ) -> Parcel {
        let mut request = Parcel::new();
        request.write_interface_token(om::DESCRIPTOR);
        request.write_string16(target);
        request.write_i32(user);
        let response = process
            .transact(om::GET_OVERLAY_INFOS_FOR_TARGET, &request, false)
            .unwrap();
        response.into_parcel()
    }

    /// OverlayManagerService.restoreSettingsLocked skips a missing base
    /// file; getOverlayInfosForTarget returns a non-null empty list.
    #[test]
    fn fresh_data_queries_have_empty_settings_without_creating_a_file() {
        let data = super::super::test_temp_dir("overlay-fresh");
        std::fs::create_dir_all(data.join("system")).unwrap();
        let host = AppHost::new(crate::test_process(super::super::SYSTEM_UID));
        super::super::configure_test_host(&host, &data, &data, &data, Box::new(|_| None));
        let manager = Arc::new(OverlayManager::new(host.clone()));
        let aim_binder_host::parcel::Binder::Local(node) =
            host.process.add_service(manager.clone())
        else {
            unreachable!()
        };
        let service = host.process.local_service(node).unwrap();
        for target in [
            Some("android"),
            Some("com.android.documentsui"),
            Some("missing.target"),
            None,
        ] {
            for user in [0, 10] {
                let response = query(&service, target, user);
                let mut reader = response.reader();
                assert_eq!(reader.read_exception(), Ok(Ok(())));
                assert_eq!(reader.read_i32(), Ok(0));
                assert_eq!(reader.remaining(), 0);
            }
        }
        assert_eq!(manager.settings(), Ok([].as_slice()));
        assert!(!data.join("system/overlays.xml").exists());
        host.process.shutdown().unwrap();
        std::fs::remove_dir_all(data).unwrap();
    }

    #[test]
    fn read_failures_are_exceptions_with_guest_paths() {
        let data = super::super::test_temp_dir("overlay-error");
        // Reading a directory fails even when tests run as a privileged user.
        std::fs::create_dir_all(data.join("system/overlays.xml")).unwrap();
        let host = AppHost::new(crate::test_process(super::super::SYSTEM_UID));
        super::super::configure_test_host(&host, &data, &data, &data, Box::new(|_| None));
        let manager = Arc::new(OverlayManager::new(host.clone()));
        let error = manager.settings().unwrap_err();
        assert!(
            error.message.starts_with("/data/system/overlays.xml:"),
            "{error:?}"
        );
        assert!(!error.message.contains(data.to_str().unwrap()), "{error:?}");
        let aim_binder_host::parcel::Binder::Local(node) =
            host.process.add_service(manager.clone())
        else {
            unreachable!()
        };
        let service = host.process.local_service(node).unwrap();
        for _ in 0..2 {
            let response = query(&service, Some("com.android.documentsui"), 0);
            let error = response.reader().read_exception().unwrap().unwrap_err();
            assert_eq!(error.code, EX_ILLEGAL_STATE);
            assert!(
                error.message.starts_with("/data/system/overlays.xml:"),
                "{error:?}"
            );
            assert!(!error.message.contains(data.to_str().unwrap()), "{error:?}");
        }
        host.process.shutdown().unwrap();
        std::fs::remove_dir_all(data).unwrap();
    }

    /// A target's overlays keep the settings' order; a static one is not
    /// mutable; constraints are read.
    #[test]
    fn restores_items_as_the_original_serializer() {
        let xml = br#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<overlays version="5">
<item packageName="a.overlay" overlayName="" userId="0" targetPackageName="com.android.documentsui" targetOverlayableName="" baseCodePath="/product/overlay/a.apk" state="6" isEnabled="true" isStatic="true" priority="1" category="" fabricated="false" />
<item packageName="b.overlay" userId="0" targetPackageName="android" baseCodePath="/b.apk" state="3" isEnabled="false" isStatic="false" priority="2" fabricated="true"><constraint type="0" value="2" /></item>
</overlays>"#;
        let items = restore(xml).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(
            (items[0].state, items[0].is_mutable, items[0].priority),
            (6, false, 1)
        );
        assert_eq!(
            (
                items[1].is_mutable,
                items[1].is_fabricated,
                items[1].constraints.clone()
            ),
            (true, true, vec![(0, 2)])
        );
        assert_eq!(items[1].overlay_name, None);
    }
}
