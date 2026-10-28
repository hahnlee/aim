//! Cold ILockSettings.generateKey/removeKey boundary, android-16.0.0_r1.
use super::*;
use aim_binder_host::parcel::{EX_SECURITY, EX_UNSUPPORTED_OPERATION};
use aim_service_aidl::dev_aim_server_isystemprocess as sp;
use std::sync::mpsc;

struct Keys(mpsc::Sender<(u32, i32, Option<String>)>);
impl Service for Keys {
    fn descriptor(&self) -> &str {
        keys::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, super::super::SYSTEM_UID);
        let mut p = Parcel::new();
        match call.code {
            keys::GENERATE_KEY => {
                let a = keys::GenerateKey::read(&mut call.data)?;
                self.0.send((call.code, a.uid, a.alias.clone())).unwrap();
                match a.alias.as_deref() {
                    Some("insecure") => {
                        p.write_exception(&service_specific(23, "Screenlock is not set"))
                    }
                    None => p.write_exception(&Exception::new(EX_NULL_POINTER, "alias is null")),
                    _ => keys::write_generate_key_reply(
                        &mut p,
                        &Some("recoverable_key:0123456789ABCDEF".into()),
                    ),
                }
            }
            keys::REMOVE_KEY => {
                let a = keys::RemoveKey::read(&mut call.data)?;
                self.0.send((call.code, a.uid, a.alias)).unwrap();
                keys::write_remove_key_reply(&mut p);
            }
            _ => panic!("unexpected key call"),
        }
        Ok(p)
    }
}

fn invoke(service: &dyn Service, uid: u32, code: u32, data: &Parcel) -> Parcel {
    service
        .transact(&mut Call {
            code,
            flags: 0,
            sender_pid: 71,
            sender_euid: uid,
            data: data.reader(),
        })
        .unwrap()
}

#[test]
fn cold_key_calls_enforce_permission_forward_identity_and_preserve_exceptions() {
    crate::system::with_discovery_binders(|host, _, _, export| {
        let access = host
            .data()
            .unwrap()
            .join("misc_de/0/apexdata/com.android.permission/access.abx");
        std::fs::create_dir_all(access.parent().unwrap()).unwrap();
        let xml = format!(
            "<access><app-id-permissions><app-id id='10100'><permission name='{RECOVER_KEYSTORE}' flags='1'/></app-id></app-id-permissions></access>"
        );
        std::fs::write(
            &access,
            aim_android_xml::abx::write(&aim_android_xml::read(xml.as_bytes()).unwrap()).unwrap(),
        )
        .unwrap();
        let permissions = super::super::permission_manager::PermissionManager::with_system(
            host.clone(),
            crate::package::permissions::AccessSystem {
                permissions: vec![crate::package::permissions::Permission {
                    name: RECOVER_KEYSTORE.into(),
                    package: "android".into(),
                    protection_level: 2,
                    dynamic: None,
                }],
                ..Default::default()
            },
        );
        assert!(host.permission_manager.set(Arc::new(permissions)).is_ok());
        let service = LockSettings::new(host.clone());
        let mut generate = Parcel::new();
        ls::GenerateKey {
            alias: Some("key".into()),
        }
        .write(&mut generate);
        let absent = invoke(&service, 10100, ls::GENERATE_KEY, &generate);
        let absent = ls::read_generate_key_reply(&mut absent.reader())
            .unwrap()
            .unwrap_err();
        assert_eq!(absent.code, EX_UNSUPPORTED_OPERATION);
        assert!(absent.message.contains("#1888"));

        let (tx, calls) = mpsc::channel();
        let bridge = export(Arc::new(Keys(tx)));
        let mut attach = Parcel::new();
        sp::AttachRecoverableKeyStoreKeys {
            keys: Some(Binder::Handle(bridge.handle)),
        }
        .write(&mut attach);
        let system = super::super::system_process::SystemProcess::new(host.clone());
        assert_eq!(
            invoke(
                &system,
                10100,
                sp::ATTACH_RECOVERABLE_KEY_STORE_KEYS,
                &attach
            )
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err()
            .code,
            EX_SECURITY
        );
        assert!(host.recoverable_keys.lock().unwrap().is_none());
        invoke(
            &system,
            1000,
            sp::ATTACH_RECOVERABLE_KEY_STORE_KEYS,
            &attach,
        )
        .reader()
        .read_exception()
        .unwrap()
        .unwrap();

        let mut remove = Parcel::new();
        ls::RemoveKey {
            alias: Some("key".into()),
        }
        .write(&mut remove);
        for (code, data) in [(ls::GENERATE_KEY, &generate), (ls::REMOVE_KEY, &remove)] {
            let denied = invoke(&service, 10101, code, data);
            let error = denied.reader().read_exception().unwrap().unwrap_err();
            assert_eq!(error.code, EX_SECURITY);
            assert!(error.message.contains(RECOVER_KEYSTORE));
            assert!(calls.try_recv().is_err());
        }
        let generated = invoke(&service, 10100, ls::GENERATE_KEY, &generate);
        assert_eq!(
            ls::read_generate_key_reply(&mut generated.reader())
                .unwrap()
                .unwrap(),
            Some("recoverable_key:0123456789ABCDEF".into())
        );
        assert_eq!(
            calls.recv().unwrap(),
            (keys::GENERATE_KEY, 10100, Some("key".into()))
        );
        for _ in 0..2 {
            let removed = invoke(&service, 10100, ls::REMOVE_KEY, &remove);
            ls::read_remove_key_reply(&mut removed.reader())
                .unwrap()
                .unwrap();
            assert_eq!(
                calls.recv().unwrap(),
                (keys::REMOVE_KEY, 10100, Some("key".into()))
            );
        }
        for (alias, expected, message) in [
            (
                Some("insecure".into()),
                EX_SERVICE_SPECIFIC,
                "Screenlock is not set",
            ),
            (None, EX_NULL_POINTER, "alias is null"),
        ] {
            let mut data = Parcel::new();
            ls::GenerateKey { alias }.write(&mut data);
            let failed = invoke(&service, 1000, ls::GENERATE_KEY, &data);
            let error = ls::read_generate_key_reply(&mut failed.reader())
                .unwrap()
                .unwrap_err();
            assert_eq!((error.code, error.message.as_str()), (expected, message));
            if expected == EX_SERVICE_SPECIFIC {
                assert_eq!(error.service_specific, 23);
            }
            calls.recv().unwrap();
        }
        let mut unsupported = Parcel::new();
        unsupported.write_interface_token(ls::DESCRIPTOR);
        let error = invoke(&service, 1000, ls::GENERATE_KEY_WITH_METADATA, &unsupported)
            .reader()
            .read_exception()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code, EX_UNSUPPORTED_OPERATION);
        assert!(error.message.contains("generateKeyWithMetadata (#1888)"));
    });
}
