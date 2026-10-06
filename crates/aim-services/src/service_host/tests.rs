//! Synchronous native package bootstrap, independent of late-service listeners.
use crate::{
    package::{owner::permission_gids::PermissionGidError, system_config::SystemConfig},
    service_host::ServiceHost,
    system::System,
};
use aim_binder_driver::{Credentials, Device, Driver, Errno, File, GuestProcess, errno, uapi::*};
use aim_binder_host::{
    local::{Call, LocalProcess, Reply, Service, Strong},
    parcel::{Binder, Exception, Parcel, UNKNOWN_TRANSACTION},
};
use aim_service_aidl::{
    dev_aim_server_ipackagebootstrapbridge as bootstrap, dev_aim_server_iservicehost as host,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

struct NoMemory;
impl GuestProcess for NoMemory {
    fn copy_from_user(&mut self, _: u64, _: &mut [u8]) -> Result<(), Errno> {
        Err(errno::EFAULT)
    }
    fn copy_to_user(&mut self, _: u64, _: &[u8]) -> Result<(), Errno> {
        Err(errno::EFAULT)
    }
    fn get_file(&mut self, _: u32) -> Result<File, Errno> {
        Err(errno::EBADF)
    }
    fn install_file(&mut self, _: File) -> Result<u32, Errno> {
        Err(errno::EBADF)
    }
    fn close_fd(&mut self, _: u32) {
        panic!("unexpected fd")
    }
}
struct Registry {
    process: Weak<LocalProcess>,
    nodes: Mutex<BTreeMap<String, Strong>>,
}
impl Service for Registry {
    fn descriptor(&self) -> &str {
        "fixture.Registry"
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::android_os_iservicemanager as sm;
        let position = call.data.position();
        if call.code == sm::CHECK_SERVICE && call.data.enforce_interface(sm::DESCRIPTOR).is_ok() {
            let name = call.data.read_string16()?.unwrap();
            assert_eq!(call.data.remaining(), 0);
            let mut reply = Parcel::new();
            sm::write_check_service_reply(
                &mut reply,
                Some(self.nodes.lock().unwrap()[&name].binder()),
            );
            return Ok(reply);
        }
        call.data.set_position(position);
        let name = call.data.read_string16()?.unwrap();
        let mut reply = Parcel::new();
        match call.code {
            1 => {
                let Some(Binder::Handle(handle)) = call.data.read_binder()? else {
                    panic!("remote node expected")
                };
                self.nodes
                    .lock()
                    .unwrap()
                    .insert(name, self.process.upgrade().unwrap().strong(handle));
            }
            2 => reply.write_binder(Some(self.nodes.lock().unwrap()[&name].binder())),
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        Ok(reply)
    }
}
fn register(process: &Arc<LocalProcess>, name: &str, node: Binder) {
    let mut p = Parcel::new();
    p.write_string16(Some(name));
    p.write_binder(Some(node));
    process.strong(0).transact(1, &p, false).unwrap();
}
fn find(process: &Arc<LocalProcess>, name: &str) -> Strong {
    let mut p = Parcel::new();
    p.write_string16(Some(name));
    let reply = process.strong(0).transact(2, &p, false).unwrap();
    let Some(Binder::Handle(handle)) = reply.reader().read_binder().unwrap() else {
        panic!("remote node expected")
    };
    process.strong(handle)
}
struct SdkFailure(Mutex<Exception>);
impl Service for SdkFailure {
    fn descriptor(&self) -> &str {
        "android.os.IInstalld"
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::android_os_iinstalld as api;
        assert_eq!(call.sender_euid, 1000);
        assert_eq!(call.code, api::RECONCILE_SDK_DATA);
        call.data.enforce_interface(api::DESCRIPTOR)?;
        assert_eq!(call.data.read_i32()?, 1);
        let start = call.data.position();
        let size = call.data.read_i32()?;
        assert_eq!(call.data.read_string16()?, None);
        assert_eq!(
            call.data.read_string16()?.as_deref(),
            Some("fixture.sdk.client")
        );
        assert_eq!(
            aim_service_aidl::read_string_list(&mut call.data)?,
            Some(vec![])
        );
        assert_eq!(call.data.read_i32()?, -1);
        assert_eq!(call.data.read_i32()?, 19001);
        assert_eq!(call.data.read_i32()?, 0);
        assert_eq!(call.data.read_string16()?.as_deref(), Some("default"));
        assert_eq!(call.data.read_i32()?, 1);
        assert_eq!(call.data.position() - start, size as usize);
        assert_eq!(call.data.remaining(), 0);
        let mut reply = Parcel::new();
        reply.write_exception(&self.0.lock().unwrap());
        Ok(reply)
    }
}
struct Owner {
    calls: Mutex<Vec<i32>>,
    reject: AtomicBool,
    bcp_reads: AtomicUsize,
    bcp_present: bool,
    signing_debuggable: AtomicBool,
    test_base_reply: AtomicUsize,
    query_reply: AtomicUsize,
    invalidations: AtomicUsize,
    permission_reply: AtomicUsize,
    permission_calls: Mutex<Vec<(String, Option<(i32, i32)>)>>,
    gid_reply: AtomicUsize,
    query_calls: Mutex<Vec<(String, i32)>>,
    malformed_bcp: AtomicBool,
    malformed_seinfo: AtomicBool,
    legacy_reply: AtomicUsize,
    domain_reply: AtomicUsize,
    users_reply: AtomicUsize,
    apex_reply: AtomicUsize,
    gid: i32,
}
impl Service for Owner {
    fn descriptor(&self) -> &str {
        bootstrap::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.sender_euid, 1000);
        call.data.enforce_interface(bootstrap::DESCRIPTOR)?;
        if call.code == bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH {
            self.bcp_reads.fetch_add(1, Ordering::SeqCst);
        }
        let mut reply = Parcel::new();
        if self.reject.load(Ordering::SeqCst) {
            reply.write_exception(&Exception::security("original owner denied"));
            return Ok(reply);
        }
        reply.write_no_exception();
        match call.code {
            bootstrap::IS_TEST_BASE_ON_BOOTCLASSPATH => {
                reply.write_bool(self.bcp_present);
                if self.malformed_bcp.load(Ordering::SeqCst) {
                    reply.write_i32(99);
                }
            }
            bootstrap::IS_SIGNING_DEBUGGABLE => {
                reply.write_bool(self.signing_debuggable.load(Ordering::SeqCst))
            }
            bootstrap::IS_SHARED_UID_MIGRATION_BEST_EFFORT => reply.write_bool(false),
            bootstrap::IS_TEST_BASE_LIBRARY_CHANGE_ENABLED => {
                let cache = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
                let parsed = crate::package::pkg::AndroidPackage::read_cache_entry(&cache).unwrap();
                let mode = self.test_base_reply.load(Ordering::SeqCst);
                if mode != 2 {
                    reply.write_bool(parsed.target_sdk_version > 29);
                }
                if mode == 1 {
                    reply.write_i32(99);
                }
            }
            bootstrap::IS_APPLICATION_QUERY_FILTERING_ENABLED
            | bootstrap::IS_DOMAIN_VERIFICATION_RESTRICTED => {
                let name = call.data.read_string16()?.unwrap();
                let sdk = call.data.read_i32()?;
                assert_eq!(call.data.remaining(), 0);
                self.query_calls.lock().unwrap().push((name, sdk));
                let mode = self.query_reply.load(Ordering::SeqCst);
                if mode != 2 {
                    reply.write_bool(
                        sdk >= if call.code == bootstrap::IS_DOMAIN_VERIFICATION_RESTRICTED {
                            31
                        } else {
                            30
                        },
                    );
                }
                if mode == 1 {
                    reply.write_i32(99);
                }
            }
            bootstrap::IS_DOMAIN_VERIFIER_UID => {
                let uid = call.data.read_i32()?; assert_eq!(call.data.remaining(), 0);
                let mode = self.query_reply.load(Ordering::SeqCst);
                if mode != 2 { reply.write_bool(uid == 10073 || uid == 1010073); }
                if mode == 1 { reply.write_i32(99); }
            }
            bootstrap::INVALIDATE_PACKAGE_INFO_CACHE => {
                assert_eq!(call.data.remaining(), 0);
                self.invalidations.fetch_add(1, Ordering::SeqCst);
                // This uses the reply fault owner already exercised for query policy.
                if self.query_reply.load(Ordering::SeqCst) == 1 { reply.write_i32(99); }
            }
            bootstrap::ARE_NATIVE_LIBRARY_DEPENDENCIES_ENFORCED => {
                assert!(
                    call.data
                        .read_string16()?
                        .is_some_and(|name| !name.is_empty())
                );
                let sdk = call.data.read_i32()?;
                reply.write_i32(i32::from(sdk >= 31));
            }
            bootstrap::GET_PACKAGE_INSTALLED_PERMISSIONS
            | bootstrap::GET_PACKAGE_GRANTED_PERMISSIONS => {
                let name = call.data.read_string16()?.unwrap();
                let identity = if call.code == bootstrap::GET_PACKAGE_GRANTED_PERMISSIONS {
                    Some((call.data.read_i32()?, call.data.read_i32()?))
                } else {
                    None
                };
                assert_eq!(call.data.remaining(), 0);
                self.permission_calls.lock().unwrap().push((name, identity));
                let mode = self.permission_reply.load(Ordering::SeqCst);
                if mode == 1 {
                    reply.write_i32(-1);
                } else {
                    reply.write_i32(if mode == 3 { 2 } else { 1 });
                    let value = if identity.is_some() {
                        "fixture.granted"
                    } else {
                        "fixture.installed"
                    };
                    reply.write_string16(if mode == 2 { None } else { Some(value) });
                    if mode == 3 {
                        reply.write_string16(Some(value));
                    }
                }
                if mode == 4 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GET_PERMISSION_GIDS_FOR_UID => {
                self.calls.lock().unwrap().push(call.data.read_i32()?);
                reply.write_i32(2);
                reply.write_i32(self.gid);
                reply.write_i32(self.gid);
                if self.gid_reply.load(Ordering::SeqCst) != 0 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GET_LEGACY_PERMISSION_STATE => {
                let app_id = call.data.read_i32()?;
                let users = aim_service_aidl::read_int_array(&mut call.data)?.unwrap();
                let mode = self.legacy_reply.load(Ordering::SeqCst);
                let mut payload = Parcel::new();
                payload.write_i32(app_id);
                payload.write_i32(users.len() as i32);
                for user in users {
                    payload.write_i32(user);
                    payload.write_bool(user == 10);
                    payload.write_i32(1);
                    payload.write_string16(None);
                    payload.write_bool(true);
                    payload.write_bool(false);
                    payload.write_i32(self.gid);
                }
                if mode == 2 {
                    payload.write_i32(99);
                }
                aim_service_aidl::write_byte_array(
                    &mut reply,
                    (mode != 1).then_some(payload.data()),
                );
                if mode == 3 {
                    reply.write_i32(99);
                }
            }
            bootstrap::NOTIFY_APEX_SCAN_RESULTS => {
                let bytes = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
                assert_eq!(bytes, 0i32.to_le_bytes());
                assert_eq!(call.data.remaining(), 0);
                if self.apex_reply.load(Ordering::SeqCst) == 4 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GET_APEX_BOOT_INVENTORY => {
                let mode = self.apex_reply.load(Ordering::SeqCst);
                let mut payload = Parcel::new();
                payload.write_i32(if mode == 1 { 0 } else { -1 });
                payload.write_i32(if mode == 1 { 0 } else { 1 });
                if mode != 1 {
                    payload.write_string16(Some("raw.module"));
                    payload.write_string16(Some("/apex/mounted"));
                    payload.write_string16(Some("/product/apex/factory.apex"));
                    payload.write_bool(false);
                    payload.write_string16(Some("/data/apex/active/updated.apex"));
                    payload.write_bool(true);
                }
                if mode == 2 {
                    payload.write_i32(99);
                }
                aim_service_aidl::write_byte_array(
                    &mut reply,
                    (mode != 3).then_some(payload.data()),
                );
                if mode == 4 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GET_PACKAGE_SCAN_USERS => {
                let mode = self.users_reply.load(Ordering::SeqCst);
                let mut payload = Parcel::new();
                payload.write_bool(mode != 1);
                if mode != 1 {
                    payload.write_i32(if mode == 2 { 0 } else { 2 });
                    if mode != 2 {
                        for (id, pre_created, adb) in [(0, false, true), (10, true, false)] {
                            payload.write_i32(id);
                            payload.write_bool(pre_created);
                            payload.write_bool(adb);
                        }
                    }
                }
                if mode == 3 {
                    payload.write_i32(99);
                }
                aim_service_aidl::write_byte_array(
                    &mut reply,
                    (mode != 4).then_some(payload.data()),
                );
                if mode == 5 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GENERATE_NEW_DOMAIN_ID => {
                let mode = self.domain_reply.load(Ordering::SeqCst);
                let id = [self.gid as u8; 16];
                aim_service_aidl::write_byte_array(
                    &mut reply,
                    match mode {
                        1 => None,
                        2 => Some(&[1; 15]),
                        3 => Some(&[1; 17]),
                        _ => Some(&id),
                    },
                );
                if mode == 4 {
                    reply.write_i32(99);
                }
            }
            bootstrap::GET_SE_INFO_TARGET_SDK_VERSION => {
                let cache = aim_service_aidl::read_byte_array(&mut call.data)?.unwrap();
                let parsed = crate::package::pkg::AndroidPackage::read_cache_entry(&cache)
                    .expect("original parsed package cache");
                reply.write_i32(parsed.target_sdk_version + 1);
                if self.malformed_seinfo.load(Ordering::SeqCst) {
                    reply.write_i32(99);
                }
            }
            _ => return Err(UNKNOWN_TRANSACTION),
        }
        assert_eq!(call.data.remaining(), 0);
        Ok(reply)
    }
}
fn attach(process: &Arc<LocalProcess>, bridge: Option<Binder>) -> Result<(), Exception> {
    let service = find(process, "host");
    // Independent Java Parcel primitive/interface-object encoding.
    let mut p = Parcel::new();
    p.write_interface_token(host::DESCRIPTOR);
    p.write_binder(bridge);
    let reply = service
        .transact(host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE, &p, false)
        .unwrap();
    host::read_attach_package_bootstrap_bridge_reply(&mut reply.reader()).unwrap()
}
fn signing_wire(details: &crate::package::sign::SigningDetails) -> Parcel {
    let mut p = Parcel::new();
    p.write_i32(1);
    p.write_bool(details.unknown);
    if details.unknown {
        return p;
    }
    p.write_i32(details.signatures.len() as i32);
    for (i, cert) in details.signatures.iter().enumerate() {
        aim_service_aidl::write_byte_array(&mut p, Some(cert));
        p.write_i32(details.current_flags.get(i).copied().unwrap_or(0));
    }
    p.write_i32(details.scheme_version);
    p.write_i32(
        details
            .public_keys
            .as_ref()
            .map_or(-1, |keys| keys.len() as i32),
    );
    if let Some(keys) = &details.public_keys {
        for key in keys {
            aim_service_aidl::write_byte_array(&mut p, key.as_deref());
        }
    }
    p.write_i32(
        details
            .past_signing_certificates
            .as_ref()
            .map_or(-1, |past| past.len() as i32),
    );
    if let Some(past) = &details.past_signing_certificates {
        for (cert, flags) in past {
            aim_service_aidl::write_byte_array(&mut p, Some(cert));
            p.write_i32(*flags);
        }
    }
    p
}
fn signing_mutation(
    process: &Arc<LocalProcess>,
    code: u32,
    details: &[&[u8]],
) -> Result<i64, Exception> {
    let mut data = Parcel::new();
    data.write_interface_token(host::DESCRIPTOR);
    for bytes in details {
        aim_service_aidl::write_byte_array(&mut data, Some(bytes));
    }
    let reply = find(process, "host").transact(code, &data, false).unwrap();
    let mut reader = reply.reader();
    reader.read_exception().unwrap()?;
    let version = reader.read_i64().unwrap();
    assert_eq!(reader.remaining(), 0);
    Ok(version)
}
fn verify_signing_transport(
    system: &Arc<System>,
    client: &Arc<LocalProcess>,
    foreign: &Arc<LocalProcess>,
    debug: bool,
) -> Arc<crate::package::sign::Overrides> {
    use crate::package::sign::{SigningDetails, read_override_details};
    let mut encoded = Parcel::new();
    encoded.write_i32(1);
    encoded.write_bool(false);
    encoded.write_i32(1);
    aim_service_aidl::write_byte_array(&mut encoded, Some(&[1, 2]));
    encoded.write_i32(23);
    encoded.write_i32(3);
    encoded.write_i32(-1);
    encoded.write_i32(1);
    aim_service_aidl::write_byte_array(&mut encoded, Some(&[3]));
    encoded.write_i32(21);
    let old = read_override_details(encoded.data()).unwrap();
    let mut unknown = Parcel::new();
    unknown.write_i32(1);
    unknown.write_bool(true);
    let signing = system.package_signing_overrides().unwrap();
    for (token, tail) in [("wrong.interface", false), (host::DESCRIPTOR, true)] {
        let mut data = Parcel::new();
        data.write_interface_token(token);
        if tail {
            data.write_i32(99);
        }
        assert!(
            find(client, "host")
                .transact(host::CLEAR_PACKAGE_SIGNING_OVERRIDES, &data, false)
                .is_err()
        );
    }
    for (code, args) in [
        (
            host::ADD_PACKAGE_SIGNING_OVERRIDE,
            vec![encoded.data(), unknown.data()],
        ),
        (host::REMOVE_PACKAGE_SIGNING_OVERRIDE, vec![encoded.data()]),
        (host::CLEAR_PACKAGE_SIGNING_OVERRIDES, vec![]),
    ] {
        assert!(signing_mutation(foreign, code, &args).is_err_and(|e| e.code == -1));
        if !debug {
            assert!(signing_mutation(client, code, &args).is_err_and(|e| e.code == -1));
        }
    }
    if debug {
        let mut trailing = encoded.data().to_vec();
        trailing.extend_from_slice(&99i32.to_le_bytes());
        for bytes in [
            trailing.as_slice(),
            &[][..],
            &[2, 0, 0, 0][..],
            &encoded.data()[..encoded.data().len() - 1],
        ] {
            assert!(
                signing_mutation(
                    client,
                    host::ADD_PACKAGE_SIGNING_OVERRIDE,
                    &[bytes, unknown.data()]
                )
                .is_err_and(|e| e.code == -3)
            );
        }
        assert_eq!(signing.snapshot().version, 1);
        assert_eq!(
            signing_mutation(
                client,
                host::ADD_PACKAGE_SIGNING_OVERRIDE,
                &[encoded.data(), unknown.data()]
            )
            .unwrap(),
            2
        );
        assert_eq!(signing.apply(&old), SigningDetails::unknown());
        assert_eq!(
            signing_mutation(
                client,
                host::REMOVE_PACKAGE_SIGNING_OVERRIDE,
                &[encoded.data()]
            )
            .unwrap(),
            3
        );
        assert_eq!(signing.apply(&old), old);
        signing_mutation(
            client,
            host::ADD_PACKAGE_SIGNING_OVERRIDE,
            &[encoded.data(), unknown.data()],
        )
        .unwrap();
        assert_eq!(
            signing_mutation(client, host::CLEAR_PACKAGE_SIGNING_OVERRIDES, &[]).unwrap(),
            5
        );
        assert_eq!(signing.apply(&old), old);
    }
    signing
}
fn capture_scan(process: &Arc<LocalProcess>) -> Result<Strong, Exception> {
    let mut data = Parcel::new();
    host::CapturePackageScan {}.write(&mut data);
    let reply = find(process, "host")
        .transact(host::CAPTURE_PACKAGE_SCAN, &data, false)
        .unwrap();
    let binder = host::read_capture_package_scan_reply(&mut reply.reader()).unwrap()?;
    let Some(Binder::Handle(handle)) = binder else {
        panic!("remote snapshot expected")
    };
    Ok(process.strong(handle))
}

fn scan_version(lease: &Strong) -> Result<i64, Exception> {
    use aim_service_aidl::dev_aim_server_ipackagescansnapshot as api;
    let mut data = Parcel::new();
    api::GetVersion {}.write(&mut data);
    let reply = lease.transact(api::GET_VERSION, &data, false).unwrap();
    api::read_get_version_reply(&mut reply.reader()).unwrap()
}

fn query_context(
    scan: &Arc<crate::package::scan_snapshot::Snapshot>,
) -> crate::package::scan_snapshot::query_state::Context {
    query_context_for(scan.owner(), scan.version())
}
fn query_context_for(
    owner: &crate::package::scan::SigningScan,
    version: u64,
) -> crate::package::scan_snapshot::query_state::Context {
    use crate::package::{
        bootstrap::{ApexInventory, ScanUsers},
        model,
        scan::User,
        scan_snapshot::query_state::{Context, PackageInputs, UserInputs},
    };
    let mut packages = BTreeMap::new();
    for (settings, factory) in [
        (&owner.settings.packages, false),
        (&owner.settings.disabled_system_packages, true),
    ] {
        for setting in settings {
            let stored = if factory {
                owner.disabled_user_states(&setting.name)
            } else {
                owner.scanned_user_states(&setting.name)
            }
            .unwrap();
            let ids: std::collections::BTreeSet<_> =
                stored.keys().copied().chain([0, 10]).collect();
            packages.insert(
                (setting.name.clone(), factory),
                PackageInputs {
                    app_id: setting.app_id,
                    path: setting.code_path.clone(),
                    version: setting.version_code,
                    installed_permissions: vec!["fixture.permission".into()],
                    domain_verification: None,
                    uri_relative_filter_groups: vec![],
                    filter_application_query: true,
                    syncable_authorities: vec![],
                    users: ids
                        .into_iter()
                        .map(|id| {
                            (
                                id,
                                UserInputs {
                                    gids: vec![7],
                                    granted_permissions: vec!["fixture.permission".into()],
                                    domain_selection: None,
                                },
                            )
                        })
                        .collect(),
                },
            );
        }
    }
    let mut retained_packages = BTreeMap::new();
    let identities = &owner.identities;
    let detached = identities.ids.owners().filter_map(|(id, _)| {
        identities
            .ids
            .detached_setting(id)
            .map(|setting| (id, setting))
    });
    let shared = identities.shared_users.values().flat_map(|group| {
        group
            .retained_settings()
            .map(|(_, setting)| (group.app_id, setting))
    });
    for (id, old) in detached.chain(shared) {
        let setting = &old.package;
        retained_packages.insert(
            (id, setting.name.clone()),
            PackageInputs {
                app_id: setting.app_id,
                path: setting.code_path.clone(),
                version: setting.version_code,
                installed_permissions: vec!["fixture.permission".into()],
                domain_verification: None,
                uri_relative_filter_groups: vec![],
                filter_application_query: true,
                syncable_authorities: vec![],
                users: old
                    .users
                    .keys()
                    .copied()
                    .chain([0, 10])
                    .map(|id| {
                        (
                            id,
                            UserInputs {
                                gids: vec![7],
                                granted_permissions: vec!["fixture.permission".into()],
                                domain_selection: None,
                            },
                        )
                    })
                    .collect(),
            },
        );
    }
    Context {
        scan_version: version,
        native_domains: None,
        nonce: Some(version as i64),
        system: model::System {
            sdk_sandbox_package: Some(None),
            ..Default::default()
        },
        platform: model::Platform::default(),
        users: [0, 10]
            .into_iter()
            .map(|id| {
                (
                    id,
                    model::User {
                        id,
                        profile_group_id: id,
                        unlocking_or_unlocked: true,
                        ..Default::default()
                    },
                )
            })
            .collect(),
        apex_inventory: ApexInventory {
            packages: Some(vec![]),
            active: vec![],
        },
        scan_users: ScanUsers {
            users: Some(
                [0, 10]
                    .into_iter()
                    .map(|id| User {
                        id,
                        pre_created: false,
                        adb_install_disallowed: false,
                    })
                    .collect(),
            ),
        },
        cross_user_suspensions: false,
        packages,
        retained_packages,
    }
}
fn query_names(process: &Arc<LocalProcess>) -> Result<Option<String>, Exception> {
    use aim_service_aidl::android_content_pm_ipackagemanagernative as api;
    let mut p = Parcel::new();
    p.write_interface_token(api::DESCRIPTOR);
    p.write_i32(1);
    p.write_i32(10100);
    let reply = find(process, "query_native")
        .transact(api::GET_NAMES_FOR_UIDS, &p, false)
        .unwrap();
    let mut r = reply.reader();
    r.read_exception().unwrap()?;
    assert_eq!(r.read_i32().unwrap(), 1);
    let name = r.read_string16().unwrap();
    assert_eq!(r.remaining(), 0);
    Ok(name)
}
fn query_uid(process: &Arc<LocalProcess>, name: &str, user: i32) -> Result<i32, Exception> {
    use aim_service_aidl::android_content_pm_ipackagemanagernative as api;
    let mut p = Parcel::new();
    api::GetPackageUid {
        package_name: Some(name.into()),
        flags: 0,
        user_id: user,
    }
    .write(&mut p);
    let reply = find(process, "query_native")
        .transact(api::GET_PACKAGE_UID, &p, false)
        .unwrap();
    let mut r = reply.reader();
    let uid = api::read_get_package_uid_reply(&mut r).unwrap()?;
    assert_eq!(r.remaining(), 0);
    Ok(uid)
}
fn replica_owner() -> crate::package::scan::SigningScan {
    use crate::package::{
        scan::{CapturedUsers, ReplicaRuntime, SigningScan},
        settings::{Package, Settings},
    };
    let mut owner = SigningScan::new(
        &SystemConfig::default(),
        &Settings {
            packages: vec![Package {
                name: "fixture".into(),
                app_id: 10100,
                ..Default::default()
            }],
            ..Default::default()
        },
        36,
    )
    .unwrap();
    owner
        .capture_user_states(BTreeMap::from([(
            ("fixture".into(), false),
            CapturedUsers {
                states: Default::default(),
                active_aliases: Default::default(),
            },
        )]))
        .unwrap();
    owner
        .capture_legacy_permissions(
            &[0],
            BTreeMap::from([(("fixture".into(), false), Default::default())]),
            owner
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Default::default()))
                .collect(),
        )
        .unwrap();
    owner
        .capture_install_permissions_fixed(BTreeMap::from([(("fixture".into(), false), true)]))
        .unwrap();
    owner
        .complete_shared_processes(
            owner
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), vec![]))
                .collect(),
        )
        .unwrap();
    owner
        .capture_replica_runtime(BTreeMap::from([(
            ("fixture".into(), false),
            ReplicaRuntime {
                usage: [0; 8],
                seinfo: None,
                override_seinfo: None,
                library_files: vec![],
                libraries: vec![],
            },
        )]))
        .unwrap();
    owner
}

struct Processes {
    driver: Arc<Driver>,
    processes: Vec<Arc<LocalProcess>>,
}
impl Drop for Processes {
    fn drop(&mut self) {
        for p in &self.processes {
            self.driver.release(p.proc_handle());
        }
    }
}
fn until(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !predicate() {
        assert!(
            Instant::now() < deadline,
            "Binder death notification did not arrive"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn synchronous_package_bootstrap_preserves_replacement_and_propagates_owner_failures() {
    exercise_bootstrap(false, false);
}
#[test]
#[ignore = "requires pinned original image; run explicitly"]
fn native_boot_scan_uses_retained_original_bootstrap_owners() {
    exercise_bootstrap(true, false);
}
#[test]
fn signing_override_transport_uses_captured_debug_policy() {
    exercise_bootstrap(false, true);
}
#[test]
#[ignore = "requires pinned original image; run explicitly"]
fn signing_override_transport_reaches_live_apk_collection() {
    exercise_bootstrap(true, true);
}
fn exercise_bootstrap(run_scan: bool, debuggable: bool) {
    let driver = Driver::new();
    let open = |pid, euid| {
        LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid,
                euid,
                security_context: None,
            },
        )
    };
    let manager = open(94001, 1000);
    let native = open(94002, 1000);
    let first = open(94003, 1000);
    let second = open(94004, 1000);
    let foreign = open(94005, 19001);
    let _processes = Processes {
        driver: driver.clone(),
        processes: vec![
            manager.clone(),
            native.clone(),
            first.clone(),
            second.clone(),
            foreign.clone(),
        ],
    };
    let registry = Arc::new(Registry {
        process: Arc::downgrade(&manager),
        nodes: Mutex::new(BTreeMap::new()),
    });
    let Binder::Local(ptr) = manager.add_service(registry) else {
        unreachable!()
    };
    let mut object = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: 0,
        binder: ptr,
        cookie: ptr,
    }
    .encode();
    driver
        .ioctl(
            manager.proc_handle(),
            94006,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    for p in &_processes.processes {
        p.start();
    }
    let system = System::new(native.clone(), &[]);
    let late = Arc::new(AtomicBool::new(false));
    let told = late.clone();
    system.add_bridge_listener(Box::new(move |_| {
        told.store(true, Ordering::SeqCst);
    }));
    let node = native.add_service(Arc::new(ServiceHost::new(native.clone(), &system)));
    register(&native, "host", node);
    let sdk_failure = Arc::new(SdkFailure(Mutex::new(Exception::new(
        aim_binder_host::parcel::EX_SERVICE_SPECIFIC,
        "userId invalid: -1",
    ))));
    register(&first, "installd", first.add_service(sdk_failure.clone()));
    for code in [0, 2, 22] {
        sdk_failure.0.lock().unwrap().service_specific = code;
        let mut request = Parcel::new();
        host::ReconcilePackageSdkData {
            volume_uuid: None,
            package_name: Some("fixture.sdk.client".into()),
            sub_dir_names: Some(vec![]),
            user_id: -1,
            app_id: 19001,
            previous_app_id: 0,
            se_info: Some("default".into()),
            flags: 1,
        }
        .write(&mut request);
        let reply = find(&first, "host")
            .transact(host::RECONCILE_PACKAGE_SDK_DATA, &request, false)
            .unwrap();
        let mut reader = reply.reader();
        let error = host::read_reconcile_package_sdk_data_reply(&mut reader)
            .unwrap()
            .unwrap_err();
        assert_eq!(error, *sdk_failure.0.lock().unwrap());
        assert_eq!(reader.remaining(), 0);
    }
    let (_, queries) = crate::package::service::PackageQueries::from_system(&system);
    register(&native, "query_native", native.add_service(queries));
    assert!(query_names(&first).is_err_and(|error| error.code == -5));
    assert!(system.package_bootstrap().is_err());
    assert!(capture_scan(&first).is_err_and(|error| error.code == -5));
    assert!(capture_scan(&foreign).is_err_and(|error| error.code == -1));
    assert!(attach(&first, None).is_err());
    assert!(
        signing_mutation(&first, host::CLEAR_PACKAGE_SIGNING_OVERRIDES, &[])
            .is_err_and(|e| e.code == -5)
    );
    let owner = Arc::new(Owner {
        bcp_reads: AtomicUsize::new(0),
        bcp_present: true,
        signing_debuggable: AtomicBool::new(debuggable),
        test_base_reply: AtomicUsize::new(0),
        query_reply: AtomicUsize::new(0),
        invalidations: AtomicUsize::new(0),
        permission_reply: AtomicUsize::new(0),
        permission_calls: Mutex::new(vec![]),
        gid_reply: AtomicUsize::new(0),
        query_calls: Mutex::new(Vec::new()),
        malformed_bcp: AtomicBool::new(false),
        malformed_seinfo: AtomicBool::new(false),
        legacy_reply: AtomicUsize::new(0),
        domain_reply: AtomicUsize::new(0),
        users_reply: AtomicUsize::new(0),
        apex_reply: AtomicUsize::new(0),
        calls: Mutex::new(vec![]),
        reject: AtomicBool::new(false),
        gid: 3003,
    });
    let node = first.add_service(owner.clone());
    assert!(attach(&foreign, None).is_err_and(|e| e.code == -1));
    let foreign_node = foreign.add_service(Arc::new(Owner {
        bcp_reads: AtomicUsize::new(0),
        bcp_present: true,
        signing_debuggable: AtomicBool::new(debuggable),
        test_base_reply: AtomicUsize::new(0),
        query_reply: AtomicUsize::new(0),
        invalidations: AtomicUsize::new(0),
        permission_reply: AtomicUsize::new(0),
        permission_calls: Mutex::new(vec![]),
        gid_reply: AtomicUsize::new(0),
        query_calls: Mutex::new(Vec::new()),
        malformed_bcp: AtomicBool::new(false),
        malformed_seinfo: AtomicBool::new(false),
        legacy_reply: AtomicUsize::new(0),
        domain_reply: AtomicUsize::new(0),
        users_reply: AtomicUsize::new(0),
        apex_reply: AtomicUsize::new(0),
        calls: Mutex::new(vec![]),
        reject: AtomicBool::new(false),
        gid: 999,
    }));
    assert!(attach(&foreign, Some(foreign_node)).is_err_and(|e| e.code == -1));
    assert!(system.package_bootstrap().is_err());
    attach(&first, Some(node)).unwrap();
    let mut wrong_token = Parcel::new();
    wrong_token.write_interface_token("wrong.interface");
    wrong_token.write_binder(Some(node));
    assert!(
        find(&first, "host")
            .transact(host::ATTACH_PACKAGE_BOOTSTRAP_BRIDGE, &wrong_token, false)
            .is_err()
    );
    let old = system.package_bootstrap().unwrap();
    let signing = verify_signing_transport(&system, &first, &foreign, debuggable);
    owner
        .signing_debuggable
        .store(!debuggable, Ordering::SeqCst);
    assert!(attach(&first, Some(node)).is_err_and(|error| error.code == -5));
    assert!(Arc::ptr_eq(&old, &system.package_bootstrap().unwrap()));
    owner.signing_debuggable.store(debuggable, Ordering::SeqCst);
    assert!(capture_scan(&first).is_err());
    let usage = || crate::package::owner::usage::Usage::new(["fixture"]);
    let incomplete = crate::package::scan::SigningScan::new(
        &SystemConfig::default(),
        &replica_owner().settings,
        36,
    )
    .unwrap();
    assert!(
        system
            .publish_package_scan(&old, None, incomplete.clone(), usage())
            .is_err()
    );
    assert!(capture_scan(&first).is_err());
    let published = system
        .publish_package_scan(&old, None, replica_owner(), usage())
        .unwrap();
    let context = query_context(&published);
    let mut bad = context.clone();
    bad.packages
        .get_mut(&("fixture".into(), false))
        .unwrap()
        .path
        .push_str("/foreign");
    let reads = owner.query_calls.lock().unwrap().len();
    assert!(old.resolve_query_context(published.owner(), bad).is_err());
    assert_eq!(owner.query_calls.lock().unwrap().len(), reads);
    let resolved = old
        .resolve_query_context(published.owner(), context.clone())
        .unwrap();
    assert_eq!(
        resolved.packages[&("fixture".into(), false)].users[&0].gids,
        [3003, 3003]
    );
    assert_eq!(
        owner.query_calls.lock().unwrap().last().unwrap(),
        &(
            "fixture".into(),
            published.owner().settings.packages[0].target_sdk_version
        )
    );
    assert_eq!(*owner.calls.lock().unwrap(), [10100, 1010100]);
    assert_eq!(
        resolved.packages[&("fixture".into(), false)].installed_permissions,
        ["fixture.installed"]
    );
    assert_eq!(
        resolved.packages[&("fixture".into(), false)].users[&0].granted_permissions,
        ["fixture.granted"]
    );
    assert!(
        owner
            .permission_calls
            .lock()
            .unwrap()
            .contains(&("fixture".into(), Some((10100, 10))))
    );
    for mode in [1, 2, 3, 4] {
        owner.permission_reply.store(mode, Ordering::SeqCst);
        assert!(old.installed_permissions("fixture").is_err());
        assert!(old.granted_permissions("fixture", 10100, 0).is_err());
        assert!(
            old.resolve_query_context(published.owner(), context.clone())
                .is_err()
        );
    }
    owner.permission_reply.store(0, Ordering::SeqCst);
    let mut uninstalled = published.owner().clone();
    uninstalled
        .capture_user_states(BTreeMap::from([(
            ("fixture".into(), false),
            crate::package::scan::CapturedUsers {
                states: BTreeMap::from([(
                    0,
                    crate::package::restrictions::UserState {
                        installed: false,
                        ..Default::default()
                    },
                )]),
                active_aliases: Default::default(),
            },
        )]))
        .unwrap();
    let inputs = query_context_for(&uninstalled, published.version());
    owner.permission_calls.lock().unwrap().clear();
    let captured = old.resolve_query_context(&uninstalled, inputs).unwrap();
    assert!(
        captured.packages[&("fixture".into(), false)].users[&0]
            .granted_permissions
            .is_empty()
    );
    assert!(
        !owner
            .permission_calls
            .lock()
            .unwrap()
            .contains(&("fixture".into(), Some((10100, 0))))
    );
    for mode in [1, 2] {
        owner.query_reply.store(mode, Ordering::SeqCst);
        assert!(
            old.resolve_query_context(published.owner(), context.clone())
                .is_err()
        );
    }
    for mode in [1, 2] {
        owner.query_reply.store(mode, Ordering::SeqCst);
        assert!(
            old.domain_verification_restricted("fixture.domains", 31)
                .is_err()
        );
    }
    owner.query_reply.store(0, Ordering::SeqCst);
    assert!(
        !old.domain_verification_restricted("fixture.domains", 30)
            .unwrap()
    );
    assert!(
        old.domain_verification_restricted("fixture.domains", 31)
            .unwrap()
    );
    assert!(old.domain_verification_restricted("", 31).is_err());
    assert!(
        old.domain_verification_restricted("fixture.domains", -1)
            .is_err()
    );
    owner.gid_reply.store(1, Ordering::SeqCst);
    assert!(matches!(
        old.permission_gids(10100, &[0]),
        Err(crate::package::owner::permission_gids::PermissionGidError::Transport(_))
    ));
    assert!(
        old.resolve_query_context(published.owner(), context)
            .is_err()
    );
    owner.gid_reply.store(0, Ordering::SeqCst);
    owner.calls.lock().unwrap().clear();
    let mut bad = query_context(&published);
    bad.packages.clear();
    assert!(
        system
            .publish_package_queries(&old, &published, bad)
            .is_err()
    );
    let mut bad = query_context(&published);
    bad.system.sdk_sandbox_package = None;
    assert!(
        system
            .publish_package_queries(&old, &published, bad)
            .is_err()
    );
    for kind in 0..5 {
        let mut bad = query_context(&published);
        let package = bad.packages.get_mut(&("fixture".into(), false)).unwrap();
        match kind {
            0 => package.app_id += 1,
            1 => package.path.push_str("/foreign"),
            2 => package.version += 1,
            3 => {
                package.users.remove(&0);
            }
            _ => {
                package.users.get_mut(&0).unwrap().domain_selection =
                    Some((true, vec![("foreign.example".into(), 2)]));
            }
        }
        assert!(
            system
                .publish_package_queries(&old, &published, bad)
                .is_err()
        );
    }
    let query_capture = system
        .publish_package_queries(&old, &published, query_context(&published))
        .unwrap();
    assert!(Arc::ptr_eq(query_capture.scan(), &published));
    assert_eq!(query_capture.state().generation, published.version());
    assert!(query_capture.state().packages["fixture"].users[&0].data_exists);
    assert_eq!(query_names(&first).unwrap(), Some("fixture".into()));
    assert!(
        system
            .publish_package_queries(&old, &published, query_context(&published))
            .is_err()
    );
    let old_lease = capture_scan(&second).unwrap();
    assert_eq!(scan_version(&old_lease).unwrap(), 1);
    assert!(
        system
            .publish_package_scan(&old, Some(&published), incomplete, usage())
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &published,
        &system.capture_package_scan().unwrap()
    ));
    assert!(
        system
            .publish_package_scan(&old, None, replica_owner(), usage())
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &query_capture,
        &system.capture_package_queries().unwrap()
    ));
    let advanced = system
        .publish_package_scan(&old, Some(&published), replica_owner(), usage())
        .unwrap();
    assert_eq!(advanced.version(), 2);
    assert!(query_names(&first).is_err_and(|error| error.code == -5));
    assert!(
        system
            .publish_package_queries(&old, &published, query_context(&published))
            .is_err()
    );
    assert!(
        system
            .publish_package_queries(&old, &advanced, query_context(&published))
            .is_err()
    );
    let mut next_context = query_context(&advanced);
    next_context
        .packages
        .get_mut(&("fixture".into(), false))
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .gids = vec![99];
    let new_query = system
        .publish_package_queries(&old, &advanced, next_context)
        .unwrap();
    assert_eq!(new_query.state().packages["fixture"].users[&0].gids, [99]);
    assert_eq!(
        query_capture.state().packages["fixture"].users[&0].gids,
        [7]
    );
    assert_eq!(query_names(&first).unwrap(), Some("fixture".into()));
    assert!(
        system
            .publish_package_scan(&old, Some(&published), replica_owner(), usage())
            .is_err()
    );
    assert_eq!(scan_version(&old_lease).unwrap(), 1);
    assert_eq!(scan_version(&capture_scan(&first).unwrap()).unwrap(), 2);
    let mut atomic_context = query_context(&advanced);
    atomic_context.scan_version += 1;
    let mut bad = atomic_context.clone();
    bad.packages.clear();
    assert!(
        system
            .publish_package_scan_with_queries(&old, Some(&advanced), replica_owner(), usage(), bad)
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &advanced,
        &system.capture_package_scan().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &new_query,
        &system.capture_package_queries().unwrap()
    ));
    let atomic = system
        .publish_package_scan_with_queries(
            &old,
            Some(&advanced),
            replica_owner(),
            usage(),
            atomic_context,
        )
        .unwrap();
    assert_eq!(atomic.scan().version(), advanced.version() + 1);
    assert!(Arc::ptr_eq(
        atomic.scan(),
        &system.capture_package_scan().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &atomic,
        &system.capture_package_queries().unwrap()
    ));
    assert_eq!(query_names(&first).unwrap(), Some("fixture".into()));
    let mut malformed = Parcel::new();
    host::CapturePackageScan {}.write(&mut malformed);
    malformed.write_i32(99);
    assert!(
        find(&first, "host")
            .transact(host::CAPTURE_PACKAGE_SCAN, &malformed, false)
            .is_err()
    );
    let mut wrong = Parcel::new();
    wrong.write_interface_token("wrong.interface");
    assert!(
        find(&first, "host")
            .transact(host::CAPTURE_PACKAGE_SCAN, &wrong, false)
            .is_err()
    );

    assert_eq!(owner.bcp_reads.load(Ordering::SeqCst), 2);
    assert!(!late.load(Ordering::SeqCst));
    let config = SystemConfig::default();
    let boot = old.resolve_boot(&config, &|_| None).unwrap();
    assert_eq!(
        boot.migration(),
        crate::package::scan::SharedUidMigration::NewInstallOnly
    );
    assert_eq!(boot.users().users.as_ref().unwrap().len(), 2);
    assert_eq!(boot.apex().active.len(), 1);
    owner.users_reply.store(1, Ordering::SeqCst);
    assert_eq!(
        old.resolve_boot(&config, &|_| None).unwrap().users().users,
        None
    );
    owner.users_reply.store(2, Ordering::SeqCst);
    assert_eq!(
        old.resolve_boot(&config, &|_| None).unwrap().users().users,
        Some(vec![])
    );
    owner.users_reply.store(3, Ordering::SeqCst);
    assert!(old.resolve_boot(&config, &|_| None).is_err());
    owner.users_reply.store(0, Ordering::SeqCst);
    owner.apex_reply.store(2, Ordering::SeqCst);
    assert!(old.resolve_boot(&config, &|_| None).is_err());
    owner.apex_reply.store(0, Ordering::SeqCst);
    if run_scan {
        verify_boot_scan(&system, &native, &first, &old, &owner, &config);
    }
    let mut parsed = crate::package::pkg::AndroidPackage {
        feature_flag_state: Some(Vec::new()),
        package_name: "fixture.package".into(),
        target_sdk_version: 36,
        uses_libraries: vec!["android.test.base".into()],
        ..Default::default()
    };
    old.library_compatibility(&config, &|_| None)
        .unwrap()
        .apply(&mut parsed, false, false, None)
        .unwrap();
    assert!(
        !parsed
            .uses_libraries
            .iter()
            .any(|name| name == "android.test.base")
    );
    assert!(
        !old.library_policy("fixture.package", 30)
            .unwrap()
            .enforce_native_dependencies
    );
    assert!(
        old.library_policy("fixture.package", 31)
            .unwrap()
            .enforce_native_dependencies
    );
    owner.calls.lock().unwrap().clear();
    assert_eq!(
        old.permission_gids(19001, &[10, 0]).unwrap(),
        [3003, 3003, 3003, 3003]
    );
    assert_eq!(*owner.calls.lock().unwrap(), [1019001, 19001]);
    let captured_permissions = old.legacy_permissions(19001, &[10, 0]).unwrap();
    assert_eq!(captured_permissions.app_id(), 19001);
    assert!(captured_permissions.user(10).unwrap().missing);
    assert!(!captured_permissions.user(0).unwrap().missing);
    assert_eq!(captured_permissions.user(11), None);
    assert_eq!(
        captured_permissions.user(10).unwrap().permissions[0].name,
        None
    );
    assert_eq!(
        captured_permissions.user(10).unwrap().permissions[0].flags,
        3003
    );
    assert!(old.legacy_permissions(19001, &[0, 0]).is_err());
    for mode in 1..=3 {
        owner.legacy_reply.store(mode, Ordering::SeqCst);
        assert!(old.legacy_permissions(19001, &[10, 0]).is_err());
    }
    owner.legacy_reply.store(0, Ordering::SeqCst);
    assert_eq!(old.new_domain_id().unwrap(), [3003i32 as u8; 16]);
    for mode in 1..=4 {
        owner.domain_reply.store(mode, Ordering::SeqCst);
        assert!(matches!(
            old.new_domain_id(),
            Err(crate::package::bootstrap::OwnerError::Transport(
                aim_binder_host::parcel::BAD_VALUE
            ))
        ));
    }
    owner.domain_reply.store(0, Ordering::SeqCst);
    old.notify_apex_scan(&[]).unwrap();
    owner.apex_reply.store(4, Ordering::SeqCst);
    assert!(matches!(
        old.notify_apex_scan(&[]),
        Err(crate::package::bootstrap::OwnerError::Transport(
            aim_binder_host::parcel::BAD_VALUE
        ))
    ));
    owner.apex_reply.store(0, Ordering::SeqCst);
    owner.reject.store(true, Ordering::SeqCst);
    assert_eq!(old.remove_test_base(&parsed, false).unwrap(), None);
    assert!(matches!(
        old.test_base_change(&parsed),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    assert!(matches!(
        old.notify_apex_scan(&[]),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    owner.reject.store(false, Ordering::SeqCst);
    let apex = old.apex_inventory().unwrap();
    assert_eq!(apex.packages, None);
    let scan = apex.scan_apexes();
    assert_eq!(scan.len(), 1);
    assert_eq!(scan[0].module_name.as_deref(), Some("raw.module"));
    assert_eq!(scan[0].partition, crate::package::scan::Partition::Product);
    assert!(scan[0].active_changed && !scan[0].factory);
    owner.apex_reply.store(1, Ordering::SeqCst);
    assert_eq!(old.apex_inventory().unwrap().packages, Some(Vec::new()));
    for mode in 2..=4 {
        owner.apex_reply.store(mode, Ordering::SeqCst);
        assert!(matches!(
            old.apex_inventory(),
            Err(crate::package::bootstrap::OwnerError::Transport(
                aim_binder_host::parcel::BAD_VALUE
            ))
        ));
    }
    owner.apex_reply.store(0, Ordering::SeqCst);
    assert_eq!(apex.active.len(), 1);
    let users = old.scan_users().unwrap();
    assert_eq!(
        users
            .users
            .as_ref()
            .unwrap()
            .iter()
            .map(|u| (u.id, u.pre_created, u.adb_install_disallowed))
            .collect::<Vec<_>>(),
        [(0, false, true), (10, true, false)]
    );
    owner.users_reply.store(1, Ordering::SeqCst);
    assert_eq!(old.scan_users().unwrap().users, None);
    owner.users_reply.store(2, Ordering::SeqCst);
    assert_eq!(old.scan_users().unwrap().users, Some(Vec::new()));
    for mode in 3..=5 {
        owner.users_reply.store(mode, Ordering::SeqCst);
        assert!(old.scan_users().is_err());
    }
    owner.users_reply.store(0, Ordering::SeqCst);
    assert_eq!(users.users.as_ref().unwrap().len(), 2);
    assert_eq!(old.remove_test_base(&parsed, false).unwrap(), None);
    parsed.target_sdk_version = 29;
    assert!(!old.test_base_change(&parsed).unwrap());
    parsed.target_sdk_version = 30;
    assert!(old.test_base_change(&parsed).unwrap());
    for mode in 1..=2 {
        owner.test_base_reply.store(mode, Ordering::SeqCst);
        assert!(old.test_base_change(&parsed).is_err());
    }
    owner.test_base_reply.store(0, Ordering::SeqCst);
    parsed.target_sdk_version = 29;
    assert_eq!(old.seinfo_target_sdk(&parsed).unwrap(), 30);
    assert_eq!(
        crate::package::scan::SeInfoCompatibility::target_sdk(old.as_ref(), &parsed).unwrap(),
        30
    );
    owner.malformed_seinfo.store(true, Ordering::SeqCst);
    assert!(matches!(
        old.seinfo_target_sdk(&parsed),
        Err(crate::package::bootstrap::SeInfoError::Transport(
            aim_binder_host::parcel::BAD_VALUE
        ))
    ));
    assert!(crate::package::scan::SeInfoCompatibility::target_sdk(old.as_ref(), &parsed).is_err());
    owner.malformed_seinfo.store(false, Ordering::SeqCst);
    assert!(old.permission_gids(19001, &[0, 0]).is_err());
    owner.reject.store(true, Ordering::SeqCst);
    assert!(matches!(
        old.apex_inventory(),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    assert!(matches!(
        old.scan_users(),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    assert!(matches!(
        old.new_domain_id(),
        Err(crate::package::bootstrap::OwnerError::Owner(_))
    ));
    assert!(matches!(
        old.seinfo_target_sdk(&parsed),
        Err(crate::package::bootstrap::SeInfoError::Owner(_))
    ));
    assert!(matches!(
        old.legacy_permissions(19001, &[10, 0]),
        Err(crate::package::owner::legacy_permissions::Error::Owner(_))
    ));
    assert!(old.library_compatibility(&config, &|_| None).is_ok());
    assert_eq!(
        owner.bcp_reads.load(Ordering::SeqCst),
        if run_scan { 3 } else { 2 }
    );
    assert!(attach(&first, Some(node)).is_err_and(|error| error.code == -1));
    assert!(Arc::ptr_eq(&old, &system.package_bootstrap().unwrap()));
    owner.reject.store(false, Ordering::SeqCst);
    owner.malformed_bcp.store(true, Ordering::SeqCst);
    assert!(attach(&first, Some(node)).is_err_and(|error| error.code == -5));
    assert!(Arc::ptr_eq(&old, &system.package_bootstrap().unwrap()));
    owner.malformed_bcp.store(false, Ordering::SeqCst);
    owner.reject.store(true, Ordering::SeqCst);
    assert!(matches!(
        old.library_policy("fixture.package", 31),
        Err(crate::package::libraries::NativePolicyError::Owner(_))
    ));
    assert!(matches!(
        old.permission_gids(19001, &[0]),
        Err(PermissionGidError::Owner(_))
    ));
    let wrong = first.add_service(Arc::new(Registry {
        process: Arc::downgrade(&first),
        nodes: Mutex::new(BTreeMap::new()),
    }));
    assert!(attach(&first, Some(wrong)).is_err());
    assert!(Arc::ptr_eq(&old, &system.package_bootstrap().unwrap()));
    let mut marker = crate::package::sign::SigningDetails::unknown();
    if debuggable {
        marker.unknown = false;
        marker.scheme_version = 7;
        signing_mutation(
            &first,
            host::ADD_PACKAGE_SIGNING_OVERRIDE,
            &[
                signing_wire(&crate::package::sign::SigningDetails::unknown()).data(),
                signing_wire(&marker).data(),
            ],
        )
        .unwrap();
    }
    let signing_version = signing.snapshot().version;
    let replacement = second.add_service(Arc::new(Owner {
        bcp_reads: AtomicUsize::new(0),
        bcp_present: false,
        signing_debuggable: AtomicBool::new(debuggable),
        test_base_reply: AtomicUsize::new(0),
        query_reply: AtomicUsize::new(0),
        invalidations: AtomicUsize::new(0),
        permission_reply: AtomicUsize::new(0),
        permission_calls: Mutex::new(vec![]),
        gid_reply: AtomicUsize::new(0),
        query_calls: Mutex::new(Vec::new()),
        malformed_bcp: AtomicBool::new(false),
        malformed_seinfo: AtomicBool::new(false),
        legacy_reply: AtomicUsize::new(0),
        domain_reply: AtomicUsize::new(0),
        users_reply: AtomicUsize::new(0),
        apex_reply: AtomicUsize::new(0),
        calls: Mutex::new(vec![]),
        reject: AtomicBool::new(false),
        gid: 3004,
    }));
    let previous_version = system.capture_package_scan().unwrap().version();
    let prior_domains = system.capture_package_domains().ok();
    let prior_domain_state = prior_domains.as_ref().map(|d| d.owner().persisted());
    attach(&second, Some(replacement)).unwrap();
    assert!(system.capture_package_domains().is_err());
    assert_eq!(
        prior_domains.as_ref().map(|d| d.owner().persisted()),
        prior_domain_state
    );
    let current = system.package_bootstrap().unwrap();
    assert!(Arc::ptr_eq(
        &signing,
        &system.package_signing_overrides().unwrap()
    ));
    assert_eq!(signing.snapshot().version, signing_version);
    assert!(signing.apply(&crate::package::sign::SigningDetails::unknown()) == marker);
    assert!(!Arc::ptr_eq(&old, &current));
    assert!(system.check_package_bootstrap(&old).is_err());
    system.check_package_bootstrap(&current).unwrap();
    assert!(capture_scan(&second).is_err());
    assert!(
        system
            .publish_package_scan(&old, None, replica_owner(), usage())
            .is_err()
    );
    assert!(
        system
            .publish_package_scan(&current, Some(&advanced), replica_owner(), usage())
            .is_err()
    );
    let replacement_snapshot = system
        .publish_package_scan(&current, None, replica_owner(), usage())
        .unwrap();
    let replacement_lease = capture_scan(&second).unwrap();
    assert_eq!(replacement_snapshot.version(), previous_version + 1);
    assert_eq!(scan_version(&old_lease).unwrap(), 1);
    assert_eq!(
        scan_version(&replacement_lease).unwrap(),
        (previous_version + 1) as i64
    );

    assert_eq!(current.remove_test_base(&parsed, true).unwrap(), None);
    assert_eq!(
        current.remove_test_base(&parsed, false).unwrap(),
        Some(false)
    );
    parsed.target_sdk_version = 30;
    assert_eq!(
        current.remove_test_base(&parsed, false).unwrap(),
        Some(true)
    );
    assert_eq!(current.new_domain_id().unwrap(), [3004i32 as u8; 16]);
    assert!(system.capture_package_domains().is_err());
    driver.release(first.proc_handle());
    // Wait for the actual old endpoint death, then ensure it did not erase the new one.
    until(|| {
        matches!(
            old.permission_gids(19001, &[0]),
            Err(PermissionGidError::Transport(_))
        )
    });
    assert!(Arc::ptr_eq(&current, &system.package_bootstrap().unwrap()));
    assert!(Arc::ptr_eq(
        &replacement_snapshot,
        &system.capture_package_scan().unwrap()
    ));
    assert_eq!(scan_version(&old_lease).unwrap(), 1);
    {
        use aim_service_aidl::dev_aim_server_ipackagescansnapshot as api;
        let mut data = Parcel::new();
        api::Close {}.write(&mut data);
        let reply = old_lease.transact(api::CLOSE, &data, false).unwrap();
        api::read_close_reply(&mut reply.reader()).unwrap().unwrap();
        assert!(scan_version(&old_lease).is_err());
    }

    assert!(matches!(
        old.legacy_permissions(19001, &[10, 0]),
        Err(crate::package::owner::legacy_permissions::Error::Transport(
            _
        ))
    ));
    assert_eq!(
        current
            .legacy_permissions(19001, &[10, 0])
            .unwrap()
            .user(10)
            .unwrap()
            .permissions[0]
            .flags,
        3004
    );
    assert_eq!(
        captured_permissions.user(10).unwrap().permissions[0].flags,
        3003
    );
    assert_eq!(current.permission_gids(19001, &[0]).unwrap(), [3004, 3004]);
    driver.release(second.proc_handle());
    until(|| system.package_bootstrap().is_err());
    assert!(system.check_package_bootstrap(&current).is_err());
    assert!(system.capture_package_scan().is_err());
    assert!(
        system
            .publish_package_scan(&current, None, replica_owner(), usage())
            .is_err()
    );
    let third = open(94007, 1000);
    let _third = Processes {
        driver: driver.clone(),
        processes: vec![third.clone()],
    };
    third.start();
    owner.reject.store(false, Ordering::SeqCst);
    let node = third.add_service(owner.clone());
    attach(&third, Some(node)).unwrap();
    let restarted = system.package_bootstrap().unwrap();
    assert!(signing.apply(&crate::package::sign::SigningDetails::unknown()) == marker);
    assert!(Arc::ptr_eq(
        &signing,
        &system.package_signing_overrides().unwrap()
    ));
    let resumed = system
        .publish_package_scan(&restarted, None, replica_owner(), usage())
        .unwrap();
    assert_eq!(resumed.version(), previous_version + 2);
    assert_eq!(
        scan_version(&capture_scan(&third).unwrap()).unwrap(),
        (previous_version + 2) as i64
    );
    driver.release(third.proc_handle());
    until(|| system.package_bootstrap().is_err());
    assert!(system.capture_package_scan().is_err());
    assert!(!late.load(Ordering::SeqCst));
}

struct NonceOwner {
    file: File,
    reject: AtomicBool,
    trailing: AtomicBool,
}
impl Service for NonceOwner {
    fn descriptor(&self) -> &str {
        aim_service_aidl::dev_aim_server_ibridge::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::dev_aim_server_ibridge as bridge;
        assert_eq!(call.sender_euid, 1000);
        call.data.enforce_interface(bridge::DESCRIPTOR)?;
        assert_eq!(call.code, bridge::GET_APPLICATION_SHARED_MEMORY);
        assert_eq!(call.data.remaining(), 0);
        let mut reply = Parcel::new();
        if self.reject.load(Ordering::SeqCst) {
            reply.write_exception(&Exception::security("shared memory denied"));
        } else {
            reply.write_no_exception();
            reply.write_i32(1); // nullable ParcelFileDescriptor presence
            reply.write_i32(0); // no comm channel
            reply.write_file(self.file.clone());
            if self.trailing.load(Ordering::SeqCst) {
                reply.write_i32(99);
            }
        }
        Ok(reply)
    }
}

#[test]
fn late_bridge_death_preserves_replacement_nonce_mapping() {
    use std::os::fd::AsFd;
    let driver = Driver::new();
    let open = |pid| {
        LocalProcess::open(
            &driver,
            Device::Binder,
            Credentials {
                pid,
                euid: 1000,
                security_context: None,
            },
        )
    };
    let manager = open(96001);
    let native = open(96002);
    let first = open(96003);
    let second = open(96004);
    let invalid = open(96005);
    let observer = open(96007);
    let _processes = Processes {
        driver: driver.clone(),
        processes: vec![
            manager.clone(),
            native.clone(),
            first.clone(),
            second.clone(),
            invalid.clone(),
            observer.clone(),
        ],
    };
    let registry = Arc::new(Registry {
        process: Arc::downgrade(&manager),
        nodes: Mutex::new(BTreeMap::new()),
    });
    let Binder::Local(ptr) = manager.add_service(registry) else {
        unreachable!()
    };
    let mut object = FlatBinderObject {
        kind: BINDER_TYPE_BINDER,
        flags: 0,
        binder: ptr,
        cookie: ptr,
    }
    .encode();
    driver
        .ioctl(
            manager.proc_handle(),
            96006,
            BINDER_SET_CONTEXT_MGR_EXT,
            &mut object,
            &mut NoMemory,
        )
        .unwrap();
    for process in &_processes.processes {
        process.start();
    }
    let system = System::new(native.clone(), &[]);
    let attaches = Arc::new(AtomicUsize::new(0));
    let observed = attaches.clone();
    system.add_bridge_listener(Box::new(move |_| {
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    let owner = |nonce| {
        let fd = crate::nonces::tests::nonce_file(nonce);
        Arc::new(NonceOwner {
            file: aim_binder_host::server::file_from_fd(fd.as_fd()).unwrap(),
            reject: AtomicBool::new(false),
            trailing: AtomicBool::new(false),
        })
    };
    let old = owner(41);
    let new = owner(42);
    register(&first, "late-old", first.add_service(old));
    register(&second, "late-new", second.add_service(new.clone()));
    register(
        &invalid,
        "late-invalid",
        invalid.add_service(Arc::new(NonceOwner {
            file: Arc::new(()),
            reject: AtomicBool::new(false),
            trailing: AtomicBool::new(false),
        })),
    );
    let old = find(&native, "late-old");
    let new_node = find(&native, "late-new");
    let invalid_node = find(&native, "late-invalid");
    let handle = |node: &Strong| {
        let Binder::Handle(handle) = node.binder() else {
            unreachable!()
        };
        handle
    };
    system.attach_bridge(handle(&old)).unwrap();
    assert_eq!(system.package_info_nonce(), Some(41));
    new.reject.store(true, Ordering::SeqCst);
    assert!(system.attach_bridge(handle(&new_node)).is_err());
    new.reject.store(false, Ordering::SeqCst);
    new.trailing.store(true, Ordering::SeqCst);
    assert!(system.attach_bridge(handle(&new_node)).is_err());
    new.trailing.store(false, Ordering::SeqCst);
    assert!(system.attach_bridge(handle(&invalid_node)).is_err());
    assert_eq!(system.package_info_nonce(), Some(41));
    assert_eq!(attaches.load(Ordering::SeqCst), 1);
    system.attach_bridge(handle(&new_node)).unwrap();
    assert_eq!(system.package_info_nonce(), Some(42));
    assert_eq!(attaches.load(Ordering::SeqCst), 2);
    system.attach_bridge(handle(&new_node)).unwrap();
    assert_eq!(system.package_info_nonce(), Some(42));
    assert_eq!(attaches.load(Ordering::SeqCst), 3);
    // The active source must keep its own Binder reference, independently of
    // this caller's temporary lookup handle and received parcel.
    drop(new_node);
    let old_died = Arc::new(AtomicBool::new(false));
    let observed = old_died.clone();
    // Binder permits one death registration per process/node reference. Observe
    // from another process, independently of the native owner's registration.
    let watched_old = find(&observer, "late-old");
    observer.link_to_death(
        &watched_old,
        Box::new(move || {
            observed.store(true, Ordering::SeqCst);
        }),
    );
    driver.release(first.proc_handle());
    until(|| old_died.load(Ordering::SeqCst));
    until(|| old.transact(0, &Parcel::new(), false).is_err());
    assert_eq!(system.package_info_nonce(), Some(42));
    // A failed replacement never registers a cleanup that owns the current map.
    driver.release(invalid.proc_handle());
    assert_eq!(system.package_info_nonce(), Some(42));
    driver.release(second.proc_handle());
    until(|| system.package_info_nonce().is_none());
}

fn verify_boot_scan(
    system: &Arc<System>,
    native: &Arc<LocalProcess>,
    client: &Arc<LocalProcess>,
    bridge: &Arc<crate::package::bootstrap::Bridge>,
    owner: &Owner,
    config: &SystemConfig,
) {
    use crate::package::{
        bootstrap::ScanPolicy,
        parse::Platform,
        scan::{AbiPolicy, NativeLibraryInstallPolicy, ScanClock},
        write::Apks,
    };
    // A fresh wrapper around the same Binder is not the captured boot owner.
    let Binder::Handle(handle) = bridge.owner.binder() else {
        unreachable!()
    };
    let foreign = Arc::new(crate::package::bootstrap::Bridge::new(native.strong(handle)).unwrap());
    assert!(system.check_package_bootstrap(&foreign).is_err());
    let original = aim_paths::original_image();
    let root = std::env::temp_dir().join(format!(
        "aim-bootstrap-scan-{}-{}",
        std::process::id(),
        owner.signing_debuggable.load(Ordering::SeqCst)
    ));
    std::fs::create_dir(&root).unwrap();
    struct Data(std::path::PathBuf);
    impl Drop for Data {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    let data = Data(root.clone());
    let framework = root.join("system/framework");
    std::fs::create_dir_all(&framework).unwrap();
    std::os::unix::fs::symlink(
        original.join("system/framework/framework-res.apk"),
        framework.join("framework-res.apk"),
    )
    .unwrap();
    let mut apks = Apks {
        signing_overrides: Some(system.package_signing_overrides().unwrap()),
        files: Box::new(move |path| Some(root.join(path.trim_start_matches('/')))),
        platform: Platform::load(&original, Default::default()).unwrap(),
    };
    let seinfo = crate::package::owner::seinfo::Policy::load(&original).unwrap();
    let abi = AbiPolicy {
        all: vec!["arm64-v8a".into()],
        bit32: vec![],
        bit64: vec!["arm64-v8a".into()],
        native32: vec![],
        native64: vec!["arm64-v8a".into()],
        force_multi_arch_match: false,
    };
    let policy = || ScanPolicy {
        certificates: Default::default(),
        seinfo: &seinfo,
        apex_parse_flags: crate::package::parse::PARSE_IS_SYSTEM_DIR,
        first_api_level: 36,
        vendor_sdk: 36,
        abi: &abi,
        preferred_abi: "arm64-v8a",
        app_lib32_install_dir: "/data/app-lib",
        platform_runtime_64bit: true,
        install: NativeLibraryInstallPolicy {
            page_size: 4096,
            extract: false,
            debuggable: false,
            compat_16kb_disabled: false,
            manifest_compat_disabled: false,
        },
        clock: ScanClock {
            current_time: 0,
            user_id: 0,
            update_time: false,
        },
        factory_test: false,
        install_user: None,
        allow_install: true,
        instant_app: false,
        virtual_preload: false,
        stopped_system_app: false,
    };
    assert!(matches!(
        system.scan_package_first_boot(&foreign, &apks, config, &|_| None, policy()),
        Err(crate::package::bootstrap::BootError::Owner(_))
    ));
    let signing = apks.signing_overrides.take().unwrap();
    assert!(matches!(
        system.scan_package_first_boot(bridge, &apks, config, &|_| None, policy()),
        Err(crate::package::bootstrap::BootError::Owner(_))
    ));
    apks.signing_overrides = Some(Arc::new(crate::package::sign::Overrides::new(false)));
    assert!(matches!(
        system.scan_package_first_boot(bridge, &apks, config, &|_| None, policy()),
        Err(crate::package::bootstrap::BootError::Owner(_))
    ));
    if signing.is_debuggable() {
        let parsed = crate::package::pkg::AndroidPackage {
            path: Some("/system/framework/framework-res.apk".into()),
            base_apk_path: Some("/system/framework/framework-res.apk".into()),
            target_sdk_version: 36,
            ..Default::default()
        };
        apks.signing_overrides = Some(signing.clone());
        let before = apks.signing_details(&parsed).unwrap();
        let old = signing_wire(&before);
        let mut absent = before.clone();
        absent.public_keys = None;
        let mut empty = before.clone();
        empty.public_keys = Some(vec![]);
        let mut known_empty = crate::package::sign::SigningDetails::unknown();
        known_empty.unknown = false;
        let mut null = before.clone();
        null.public_keys = Some(vec![None]);
        let mut mixed = before.clone();
        mixed.public_keys.as_mut().unwrap().push(None);
        let collect = || {
            apks.collect_signing_details(
                &parsed,
                crate::package::write::CertificateCollection {
                    saved: None,
                    database_version: 3,
                    force_collect: true,
                    skip_verify: false,
                    pre_n_mr1_upgrade: false,
                },
            )
            .unwrap()
        };
        for replacement in [
            absent,
            empty,
            before.clone(),
            crate::package::sign::SigningDetails::unknown(),
            known_empty,
            null,
            mixed,
        ] {
            let new = signing_wire(&replacement);
            signing_mutation(
                client,
                host::ADD_PACKAGE_SIGNING_OVERRIDE,
                &[old.data(), new.data()],
            )
            .unwrap();
            assert!(
                collect() == replacement,
                "Binder signing owner differs in live collection"
            );
            signing_mutation(client, host::REMOVE_PACKAGE_SIGNING_OVERRIDE, &[old.data()]).unwrap();
            assert!(collect() == before);
            signing_mutation(
                client,
                host::ADD_PACKAGE_SIGNING_OVERRIDE,
                &[old.data(), new.data()],
            )
            .unwrap();
            signing_mutation(client, host::CLEAR_PACKAGE_SIGNING_OVERRIDES, &[]).unwrap();
            assert!(collect() == before);
        }
    }
    apks.signing_overrides = Some(signing);
    // Capture valid inventory before testing notification failure.
    owner.apex_reply.store(1, Ordering::SeqCst);
    let boot = bridge.resolve_boot(config, &|_| None).unwrap();
    owner.apex_reply.store(4, Ordering::SeqCst);
    assert!(
        matches!(boot.scan_first_boot(&apks, policy()), Err(crate::package::bootstrap::BootError::Scan(crate::package::scan::SigningError::Rejected(e))) if e.phase == "apex-notification")
    );
    owner.apex_reply.store(1, Ordering::SeqCst);
    let scan = system
        .scan_package_first_boot(bridge, &apks, config, &|_| None, policy())
        .unwrap();
    assert_eq!(scan.packages.len(), 1);
    assert_eq!(scan.packages[0].candidate.record.settings.name, "android");
    assert_eq!(
        scan.owner
            .scanned_user_states("android")
            .unwrap()
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [0]
    );
    let mut users = BTreeMap::from([(
        "android".into(),
        scan.owner.scanned_user_states("android").unwrap().clone(),
    )]);
    users
        .get_mut("android")
        .unwrap()
        .get_mut(&0)
        .unwrap()
        .enabled = 2;
    users.get_mut("android").unwrap().insert(
        10,
        crate::package::restrictions::UserState {
            installed: false,
            hidden: true,
            ..crate::package::restrictions::UserState::initialized()
        },
    );
    let mut restored_settings = scan.owner.settings.clone();
    restored_settings.packages[0].domain_set_id =
        Some("11111111-1111-4111-8111-111111111111".into());
    let mut restarted =
        crate::package::scan::SigningScan::new(config, &restored_settings, 36).unwrap();
    let prior = restarted.clone();
    let resources = crate::package::owner::resources::CodeResources::with_system(
        system.clone(),
        data.0.join("data"),
        None,
    );
    let stubs = std::collections::BTreeSet::new();
    let incremental = std::collections::BTreeSet::new();
    let saved = || crate::package::scan::SavedSystemScanInputs {
        users: &users,
        first_boot_or_upgrade: false,
        old_stub_packages: &stubs,
        incremental_packages: &incremental,
        resources: &resources,
    };
    assert!(matches!(
        system.scan_package_saved_system(
            &foreign,
            &mut restarted,
            &apks,
            config,
            &|_| None,
            policy(),
            saved()
        ),
        Err(crate::package::bootstrap::BootError::Owner(_))
    ));
    assert_eq!(restarted, prior);
    owner.domain_reply.store(1, Ordering::SeqCst);
    assert!(
        matches!(system.scan_package_saved_system(bridge, &mut restarted, &apks, config, &|_| None, policy(), saved()),
        Err(crate::package::bootstrap::BootError::Scan(crate::package::scan::SigningError::Rejected(e))) if e.phase == "domain")
    );
    assert_eq!(restarted, prior);
    owner.domain_reply.store(0, Ordering::SeqCst);
    let phase = system
        .scan_package_saved_system(
            bridge,
            &mut restarted,
            &apks,
            config,
            &|_| None,
            policy(),
            saved(),
        )
        .unwrap();
    assert!(phase.apex.is_empty());
    assert_eq!(phase.system.packages.len(), 1);
    assert!(phase.system.retained_data.is_empty());
    assert_eq!(
        restarted.settings.packages[0].app_id,
        restored_settings.packages[0].app_id
    );
    assert_eq!(
        restarted.settings.packages[0].domain_set_id.as_deref(),
        Some("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb")
    );
    assert_eq!(prior.settings, restored_settings);
    assert_eq!(
        restarted.scanned_user_states("android").unwrap(),
        &users["android"]
    );
    assert!(!prior.loaded_packages().contains_key("android"));
    assert!(restarted.loaded_packages().contains_key("android"));

    let mut complete =
        crate::package::scan::SigningScan::new(config, &restored_settings, 36).unwrap();
    let unscanned = complete.clone();
    assert!(matches!(
        system.scan_package_saved_boot(
            &foreign,
            &mut complete,
            &apks,
            config,
            &|_| None,
            policy(),
            saved(),
            &[],
            &stubs,
            &|_| Ok(false),
            &BTreeMap::new(),
        ),
        Err(crate::package::bootstrap::BootError::Owner(_))
    ));
    assert_eq!(complete, unscanned);
    let completed = system
        .scan_package_saved_boot(
            bridge,
            &mut complete,
            &apks,
            config,
            &|_| None,
            policy(),
            saved(),
            &[],
            &stubs,
            &|_| Ok(false),
            &BTreeMap::new(),
        )
        .unwrap();
    assert_eq!(completed.system.system.packages.len(), 1);
    assert!(completed.data.packages.is_empty());
    assert!(completed.data.recovered.is_empty());
    assert!(completed.data.rejected.is_empty());
    assert!(completed.data.removed.is_empty());
    assert_eq!(
        complete.scanned_user_states("android"),
        Some(&users["android"])
    );

    // Complete actual native parsed-code results through the daemon publisher.
    let usage = crate::package::owner::usage::Usage::new(["android"]);
    let base = system.capture_package_scan().unwrap();
    assert!(
        system
            .complete_package_scan(
                bridge,
                Some(&base),
                complete.clone(),
                usage.clone(),
                BTreeMap::new()
            )
            .is_err()
    );
    assert!(Arc::ptr_eq(&base, &system.capture_package_scan().unwrap()));
    complete
        .capture_legacy_permissions(
            &[0, 10],
            BTreeMap::from([(("android".into(), false), Default::default())]),
            complete
                .identities
                .shared_users
                .keys()
                .map(|name| (name.clone(), Default::default()))
                .collect(),
        )
        .unwrap();
    complete
        .capture_install_permissions_fixed(BTreeMap::from([(("android".into(), false), false)]))
        .unwrap();
    complete
        .complete_shared_processes(
            complete
                .identities
                .shared_users
                .iter()
                .map(|(name, group)| {
                    assert!(group.member_count() <= 1);
                    let members: Vec<_> = complete
                        .settings
                        .packages
                        .iter()
                        .filter(|setting| setting.shared_app_id() == Some(group.app_id))
                        .map(|setting| setting.name.clone())
                        .collect();
                    assert_eq!(members.len(), group.member_count());
                    (name.clone(), members)
                })
                .collect(),
        )
        .unwrap();
    let untouched = complete.clone();
    owner.reject.store(true, Ordering::SeqCst);
    assert!(
        system
            .complete_package_scan(
                bridge,
                Some(&base),
                complete.clone(),
                usage.clone(),
                BTreeMap::new()
            )
            .is_err()
    );
    assert_eq!(complete, untouched);
    assert!(Arc::ptr_eq(&base, &system.capture_package_scan().unwrap()));
    owner.reject.store(false, Ordering::SeqCst);
    let mut context = query_context_for(&complete, base.version() + 1);
    use crate::package::domain_verification::owner::Owner as DomainOwner;
    let boot_domains = bridge.boot_domains(&complete, config).unwrap();
    assert_eq!(boot_domains.changes.len(), complete.settings.packages.len());
    let domains = boot_domains.owner;
    let policies = bridge.domain_policies(&complete).unwrap();
    let before_domains = complete.clone();
    let mut missing_signing = complete.clone();
    missing_signing.settings.packages[0].signatures = None;
    assert!(bridge.boot_domains(&missing_signing, config).is_err());
    let mut changed_signing = complete.clone();
    changed_signing.settings.packages[0]
        .signatures
        .as_mut()
        .unwrap()
        .signatures = vec![vec![0]];
    assert!(bridge.boot_domains(&changed_signing, config).is_err());
    assert_eq!(complete, before_domains);
    // Supplied values must be overwritten by the native attached owner.
    context
        .packages
        .get_mut(&("android".into(), false))
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .domain_selection = Some((false, vec![("foreign.example".into(), 2)]));
    let mut changed = complete.clone();
    changed.settings.packages[0].domain_set_id =
        Some("00000000-0000-0000-0000-000000000000".into());
    assert!(
        context
            .clone()
            .resolve_domains(&changed, &domains, config, &policies)
            .is_err()
    );
    assert!(
        context
            .clone()
            .resolve_domains(
                &complete,
                &DomainOwner::new(Default::default(), Default::default()),
                config,
                &policies
            )
            .is_err()
    );
    assert!(
        context
            .clone()
            .resolve_domains(&complete, &domains, config, &BTreeMap::new())
            .is_err()
    );
    context = bridge
        .resolve_boot_domain_query_context(&complete, context, config)
        .unwrap();
    assert_eq!(
        context.packages[&("android".into(), false)].users[&0]
            .domain_selection
            .as_ref()
            .unwrap()
            .0,
        true
    );
    let mut forged = context.clone();
    forged
        .packages
        .get_mut(&("android".into(), false))
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .domain_selection = Some((false, vec![]));
    let old_capture = system.capture_package_queries().unwrap();
    assert!(
        system
            .complete_package_scan_with_queries(
                bridge,
                Some(&base),
                complete.clone(),
                usage.clone(),
                BTreeMap::new(),
                forged
            )
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &old_capture,
        &system.capture_package_queries().unwrap()
    ));
    assert!(system.capture_package_domains().is_err());
    let mut bad = context.clone();
    bad.packages.remove(&("android".into(), false));
    let old_query = system.capture_package_queries().unwrap();
    assert!(
        system
            .complete_package_scan_with_queries(
                bridge,
                Some(&base),
                complete.clone(),
                usage.clone(),
                BTreeMap::new(),
                bad
            )
            .is_err()
    );
    assert!(Arc::ptr_eq(&base, &system.capture_package_scan().unwrap()));
    assert!(Arc::ptr_eq(
        &old_query,
        &system.capture_package_queries().unwrap()
    ));
    for malformed in [
        &owner.query_reply,
        &owner.gid_reply,
        &owner.permission_reply,
    ] {
        malformed.store(1, Ordering::SeqCst);
        assert!(
            system
                .complete_package_scan_with_queries(
                    bridge,
                    Some(&base),
                    complete.clone(),
                    usage.clone(),
                    BTreeMap::new(),
                    context.clone()
                )
                .is_err()
        );
        assert!(Arc::ptr_eq(&base, &system.capture_package_scan().unwrap()));
        assert!(Arc::ptr_eq(
            &old_query,
            &system.capture_package_queries().unwrap()
        ));
        malformed.store(0, Ordering::SeqCst);
    }
    let query = system
        .complete_package_scan_with_domains(
            bridge,
            Some(&base),
            complete,
            usage,
            BTreeMap::new(),
            context,
            config,
        )
        .unwrap();
    let retained_domains = system.capture_package_domains().unwrap();
    assert!(Arc::ptr_eq(query.domains().unwrap(), &retained_domains));
    assert_eq!(
        retained_domains.changes().len(),
        query.scan().owner().settings.packages.len()
    );
    assert!(retained_domains.owner().package("android").is_some());
    use crate::package::domain_verification::enforcer::Operation;
    assert!(system.authorize_package_domain(bridge, &query, 1, 1000, Operation::Info).unwrap());
    assert!(system.authorize_package_domain(bridge, &query, 1, 0, Operation::UserQuery("android", 0)).unwrap());
    assert!(system.authorize_package_domain(bridge, &query, 1, 10001, Operation::Internal).is_err_and(|e| e.code == -1));
    assert!(system.authorize_package_domain(bridge, &old_query, 1, 1000, Operation::Info).is_err());
    let published = query.scan().clone();
    assert_eq!(published.version(), base.version() + 1);
    let mut forged = query_context_for(published.owner(), published.version() + 1)
        .resolve_domains(
            published.owner(),
            retained_domains.owner(),
            config,
            &policies,
        )
        .unwrap();
    forged.native_domains = Some(retained_domains.clone());
    forged
        .packages
        .get_mut(&("android".into(), false))
        .unwrap()
        .users
        .get_mut(&0)
        .unwrap()
        .domain_selection = Some((false, vec![]));
    assert!(
        system
            .publish_package_scan_with_queries(
                bridge,
                Some(&published),
                published.owner().clone(),
                published.usage().clone(),
                forged
            )
            .is_err()
    );
    assert!(Arc::ptr_eq(
        &published,
        &system.capture_package_scan().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &query,
        &system.capture_package_queries().unwrap()
    ));
    assert!(Arc::ptr_eq(
        &retained_domains,
        &system.capture_package_domains().unwrap()
    ));
    assert!(published.owner().loaded_packages().contains_key("android"));
    let bytes =
        crate::package::scan_snapshot::runtime_record::captured(&published, "android", false)
            .unwrap()
            .unwrap();
    assert!(!bytes.is_empty());
    assert_eq!(
        published.owner().scanned_user_states("android"),
        Some(&users["android"])
    );
    assert!(Arc::ptr_eq(
        &published,
        &system.capture_package_scan().unwrap()
    ));
    assert_eq!(
        query.state().packages["android"].installed_permissions,
        ["fixture.installed"]
    );
    assert_eq!(
        query.state().packages["android"].users[&0].granted_permissions,
        ["fixture.granted"]
    );
    assert_eq!(
        query.state().packages["android"].users[&0].gids,
        [owner.gid, owner.gid]
    );
    assert_eq!(
        query.state().packages["android"].filter_application_query,
        Some(
            published
                .owner()
                .settings
                .packages
                .iter()
                .find(|p| p.name == "android")
                .unwrap()
                .target_sdk_version
                >= 30
        )
    );
    // A real disk store backs the registry's domain compare-and-swap.
    let root = std::env::temp_dir().join(format!(
        "aim-domain-commit-{}-{}",
        std::process::id(),
        published.version()
    ));
    struct DomainData(std::path::PathBuf);
    impl Drop for DomainData {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).unwrap();
        }
    }
    std::fs::create_dir(&root).unwrap();
    let data = DomainData(root);
    std::fs::create_dir(data.0.join("system")).unwrap();
    use aim_android_xml::{Element, Node, Value};
    let mut document = aim_android_xml::read(b"<packages/>").unwrap();
    for setting in &published.owner().settings.packages {
        let mut package = Element {
            name: "package".into(),
            attrs: vec![
                ("name".into(), Value::String(setting.name.clone())),
                ("codePath".into(), Value::String(setting.code_path.clone())),
                ("userId".into(), Value::Int(setting.app_id)),
                ("version".into(), Value::Long(setting.version_code)),
                (
                    "domainSetId".into(),
                    Value::String(setting.domain_set_id.clone().unwrap()),
                ),
            ],
            content: vec![],
        };
        let signatures = setting.signatures.as_ref().unwrap();
        let mut sigs = Element {
            name: "sigs".into(),
            attrs: vec![
                (
                    "count".into(),
                    Value::Int(signatures.signatures.len() as i32),
                ),
                (
                    "schemeVersion".into(),
                    Value::Int(signatures.scheme_version),
                ),
            ],
            content: vec![],
        };
        for (index, bytes) in signatures.signatures.iter().enumerate() {
            sigs.content.push(Node::Element(Element {
                name: "cert".into(),
                attrs: vec![
                    ("index".into(), Value::Int(index as i32)),
                    (
                        "key".into(),
                        Value::String(bytes.iter().map(|b| format!("{b:02x}")).collect()),
                    ),
                ],
                content: vec![],
            }));
        }
        package.content.push(Node::Element(sigs));
        document.content.push(Node::Element(package));
    }
    let document = crate::package::owner::domains::replace(
        &document,
        &published.owner().settings.domain_verification,
    )
    .unwrap();
    let path = data.0.join("system/packages.xml");
    std::fs::write(&path, aim_android_xml::abx::write(&document).unwrap()).unwrap();
    let mut persistence = crate::package::owner::Store::open(&data.0, &[])
        .unwrap()
        .unwrap();
    let mut updated = retained_domains.owner().clone();
    updated
        .set_link_handling_internal(Some("android"), false, 0, &[0, 10])
        .unwrap();
    let first_update = query.prepare_domain_update(updated.clone()).unwrap();
    let stale_update = query.prepare_domain_update(updated).unwrap();
    let before_invalidations = owner.invalidations.load(Ordering::SeqCst);
    let committed = system
        .commit_package_domains(bridge, first_update, &mut persistence)
        .unwrap();
    assert_eq!(committed.scan().version(), published.version() + 1);
    assert!(
        !committed.state().packages["android"].users[&0]
            .domain_selection
            .as_ref()
            .unwrap()
            .0
    );
    assert!(Arc::ptr_eq(
        &committed,
        &system.capture_package_queries().unwrap()
    ));
    assert_eq!(
        persistence.state().settings.domain_verification,
        committed.domains().unwrap().owner().persisted()
    );
    assert_eq!(
        owner.invalidations.load(Ordering::SeqCst),
        before_invalidations + 1
    );
    let bytes = std::fs::read(&path).unwrap();
    assert!(
        !system
            .commit_package_domains(bridge, stale_update, &mut persistence)
            .err()
            .unwrap()
            .committed
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert!(Arc::ptr_eq(
        &committed,
        &system.capture_package_queries().unwrap()
    ));
    let mut updated = committed.domains().unwrap().owner().clone();
    updated
        .set_link_handling_internal(Some("android"), true, 0, &[0, 10])
        .unwrap();
    let conflict = committed.prepare_domain_update(updated).unwrap();
    std::fs::write(&path, b"<packages external='writer'/>").unwrap();
    assert!(
        !system
            .commit_package_domains(bridge, conflict, &mut persistence)
            .err()
            .unwrap()
            .committed
    );
    assert!(Arc::ptr_eq(
        &committed,
        &system.capture_package_queries().unwrap()
    ));
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"<packages external='writer'/>"
    );
    assert_eq!(
        owner.invalidations.load(Ordering::SeqCst),
        before_invalidations + 1
    );
    std::fs::write(&path, &bytes).unwrap();
    let mut updated = committed.domains().unwrap().owner().clone();
    updated
        .set_link_handling_internal(Some("android"), true, 0, &[0, 10])
        .unwrap();
    let update = committed.prepare_domain_update(updated).unwrap();
    owner.query_reply.store(1, Ordering::SeqCst);
    let error = system
        .commit_package_domains(bridge, update, &mut persistence)
        .err()
        .unwrap();
    assert!(error.committed);
    let cache_failed = system.capture_package_queries().unwrap();
    assert_eq!(
        cache_failed.scan().version(),
        committed.scan().version() + 1
    );
    assert!(
        cache_failed.state().packages["android"].users[&0]
            .domain_selection
            .as_ref()
            .unwrap()
            .0
    );
    owner.query_reply.store(0, Ordering::SeqCst);
    bridge.invalidate_package_info_cache().unwrap();
    assert!(
        query.state().packages["android"].users[&0]
            .domain_selection
            .as_ref()
            .unwrap()
            .0
    );
    let android = &query.state().packages["android"];
    let loaded = &published.owner().loaded_packages()["android"];
    assert_eq!(android.pkg.as_deref(), Some(&loaded.package));
    assert_eq!(
        android.signatures,
        published
            .owner()
            .settings
            .packages
            .iter()
            .find(|s| s.name == "android")
            .unwrap()
            .signatures
    );
    assert_eq!(android.pkg.as_ref().unwrap().uid, 1000);
    assert_eq!(query_uid(client, "android", 0).unwrap(), 1000);
    let lease = capture_scan(client).unwrap();
    assert_eq!(scan_version(&lease).unwrap(), cache_failed.scan().version() as i64);
    {
        use aim_service_aidl::dev_aim_server_ipackagescansnapshot as api;
        let mut data = Parcel::new();
        api::GetCodeLength {
            package_name: Some("android".into()),
            disabled: false,
        }
        .write(&mut data);
        let reply = lease.transact(api::GET_CODE_LENGTH, &data, false).unwrap();
        assert!(
            api::read_get_code_length_reply(&mut reply.reader())
                .unwrap()
                .unwrap()
                > 0
        );
    }

    owner.reject.store(true, Ordering::SeqCst);
    assert!(matches!(
        system.scan_package_first_boot(bridge, &apks, config, &|_| None, policy()),
        Err(crate::package::bootstrap::BootError::Owner(_))
    ));
    owner.reject.store(false, Ordering::SeqCst);
    owner.apex_reply.store(0, Ordering::SeqCst);
    drop(data);
}

#[test]
fn sdk_data_host_rejects_foreign_callers_bad_tokens_tails_and_missing_owner() {
    use aim_binder_host::parcel::{BAD_VALUE, Reader};
    let driver = Driver::new();
    let process = LocalProcess::open(
        &driver,
        Device::Binder,
        Credentials {
            pid: 97001,
            euid: 1000,
            security_context: None,
        },
    );
    let system = System::new(process.clone(), &[]);
    let host = ServiceHost::new(process.clone(), &system);
    drop(system);
    let args = host::ReconcilePackageSdkData {
        volume_uuid: None,
        package_name: Some("fixture.sdk.client".into()),
        sub_dir_names: Some(vec![Some("sdk-a".into())]),
        user_id: 0,
        app_id: 19001,
        previous_app_id: 0,
        se_info: Some("default".into()),
        flags: 3,
    };
    let mut data = Parcel::new();
    args.write(&mut data);
    for (uid, expected) in [(1000, -5), (19001, -1)] {
        let reply = host
            .transact(&mut Call {
                code: host::RECONCILE_PACKAGE_SDK_DATA,
                flags: 0,
                sender_pid: 97002,
                sender_euid: uid,
                data: Reader::new(data.data(), &[]),
            })
            .unwrap();
        let error =
            host::read_reconcile_package_sdk_data_reply(&mut Reader::new(reply.data(), &[]))
                .unwrap()
                .unwrap_err();
        assert_eq!(error.code, expected);
    }
    data.write_i32(0);
    assert!(matches!(
        host.transact(&mut Call {
            code: host::RECONCILE_PACKAGE_SDK_DATA,
            flags: 0,
            sender_pid: 97002,
            sender_euid: 1000,
            data: Reader::new(data.data(), &[]),
        }),
        Err(BAD_VALUE)
    ));
    let mut wrong = Parcel::new();
    wrong.write_interface_token("fixture.Wrong");
    assert!(
        host.transact(&mut Call {
            code: host::RECONCILE_PACKAGE_SDK_DATA,
            flags: 0,
            sender_pid: 97002,
            sender_euid: 1000,
            data: Reader::new(wrong.data(), &[]),
        })
        .is_err()
    );
    driver.release(process.proc_handle());
}
