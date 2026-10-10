//! Original ART → native DomainQueries → FD-backed Info → original creator.
use super::*;
#[path = "../../tests/common/java.rs"]
mod java;
#[path = "../../tests/common/runtime.rs"]
mod runtime;
use runtime::{Boot, Data, run};
use std::{fs, process::Command};

struct DomainSetEcho(Weak<LocalProcess>);
impl Service for DomainSetEcho {
    fn descriptor(&self) -> &str {
        aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::DESCRIPTOR
    }
    fn accepts_fds(&self) -> bool {
        true
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        use aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager as api;
        if call.code == 1717 {
            use aim_binder_host::parcel::{BAD_VALUE, Binder};
            let Some(Binder::Handle(handle)) = call.data.read_binder()? else {return Err(BAD_VALUE)};
            assert_eq!(call.data.remaining(), 0);
            let process = self.0.upgrade().ok_or(BAD_VALUE)?;
            assert_eq!(process.transact(handle, 1, &Parcel::new(), false).err(), Some(UNKNOWN_TRANSACTION));
            let reply = process.transact(handle, 2, &Parcel::new(), false)?;
            let exception = reply.reader().read_exception()?.unwrap_err();
            assert_eq!(exception.code, aim_binder_host::parcel::EX_ILLEGAL_ARGUMENT);
            assert_eq!(exception.message, "invalid probe pattern");
            let mut reply = Parcel::new(); reply.write_no_exception(); reply.write_i32(UNKNOWN_TRANSACTION); return Ok(reply);
        }
        if call.code != api::SET_DOMAIN_VERIFICATION_STATUS {
            return Err(UNKNOWN_TRANSACTION);
        }
        let args = api::SetDomainVerificationStatus::<
            crate::package::domain_verification::domain_set::DomainSet,
        >::read(&mut call.data)?;
        assert_eq!(call.data.remaining(), 0);
        let process = self.0.upgrade().ok_or(aim_binder_host::parcel::BAD_VALUE)?;
        let value = args.domains.unwrap();
        let hosts = value.resolve(&process)?;
        let count = hosts.len();
        let expected = if args.state == 4000 {
            (0..4000)
                .map(|i| Some(format!("h{i}.example")))
                .collect::<std::collections::BTreeSet<_>>()
        } else if args.state == -1 {
            std::collections::BTreeSet::from([
                None,
                Some("".into()),
                Some("Aa".into()),
                Some("BB".into()),
            ])
        } else {
            (0..args.state)
                .map(|i| Some(format!("h{i}.example")))
                .collect()
        };
        assert_eq!(
            hosts.into_iter().collect::<std::collections::BTreeSet<_>>(),
            expected
        );
        let mut reply = Parcel::new();
        api::write_set_domain_verification_status_reply(&mut reply, count as i32);
        Ok(reply)
    }
}

fn publish_fixture_domains(system: &Arc<System>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        config: &SystemConfig, persistence: &Arc<Mutex<crate::package::owner::Store>>, count: usize) {
        let base = system.capture_package_scan().unwrap();
        let mut candidate = base.owner().clone();
        // Only fixture metadata changes; its original APK and extracted tree stay read-only.
        candidate.add_fixture_domains("android", count);
        let mut user = candidate.scanned_user_states("android").unwrap()[&0].clone();
        user.installed = true; user.enabled = 1;
        candidate.set_user_state("android", 0, user).unwrap();
        let id = candidate.settings.packages.iter().find(|p| p.name == "android").unwrap().domain_set_id.clone().unwrap();
        candidate.settings.domain_verification.active.retain(|p| p.name != "android");
        candidate.settings.domain_verification.active.push(crate::package::domain_verification::Package {
            name: "android".into(), id, has_auto_verify_domains: true, signature: None,
            domains: vec![(Some("h0.example".into()), 1)], users: vec![], uri_relative_filter_groups: vec![],
        });
        let seinfo =
            crate::package::owner::seinfo::Policy::load(&aim_paths::original_image()).unwrap();
        candidate
            .assign_seinfo_at_boot(&seinfo, &mut |code| {
                bridge
                    .seinfo_target_sdk(code)
                    .map_err(|error| format!("{error:?}"))
            })
            .unwrap();
        let orders = candidate
            .identities
            .shared_users
            .iter()
            .map(|(name, group)| {
                let members = candidate
                    .settings
                    .packages
                    .iter()
                    .filter(|setting| setting.shared_app_id() == Some(group.app_id))
                    .map(|setting| setting.name.clone())
                    .collect();
                (name.clone(), members)
            })
            .collect();
        candidate.complete_shared_processes(orders).unwrap();
        let previous = system.capture_package_queries().unwrap();
        let mut context = super::query_context_for(&candidate, base.version() + 1).with_boot_classpath(&runtime::cohort::load().expect("NOT RUN: pinned native image required").image(runtime::cohort::Variant::Native).to_path_buf()).unwrap();
        context.system.uninstall_blocks = previous.context().system.uninstall_blocks.clone();
        system
            .complete_package_scan_with_domains(
                bridge,
                Some(&base),
                candidate,
                base.usage().clone(),
                BTreeMap::new(),
                context,
                config,
            )
            .unwrap();
        persistence.lock().unwrap().commit_domains(&system.capture_package_queries().unwrap().domains().unwrap().owner().persisted()).unwrap();
        let capture = system.capture_package_queries().unwrap();
        let mut domains = capture.domains().unwrap().owner().clone();
        let mut group = crate::package::intent_filter::UriRelativeFilterGroup::new(1);
        for (part, pattern, value) in [(0, 0, "/path"), (1, 0, "q=1"), (2, 1, "fragment"), (0, 1, "😀"), (0, 0, "Aa"), (0, 0, "BB")] { group.add(part, pattern, value); }
        domains.set_uri_groups("android", &[("h0.example".into(), Some(vec![group]))]).unwrap();
        system.commit_package_domains(bridge, capture.prepare_domain_update(domains).unwrap(), &mut persistence.lock().unwrap()).unwrap();
        let before_selection = system.capture_package_queries().unwrap();
        let saved_domains = persistence.lock().unwrap().state().settings.domain_verification.clone();
        let mut request = Parcel::new();
        use aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager as domain_api;
        request.write_interface_token(domain_api::DESCRIPTOR);
        request.write_string16(Some(&before_selection.domains().unwrap().owner().package("android").unwrap().id));
        request.write_i32(1); request.write_bool(false); request.write_i32(1); request.write_string16(Some("h0.example")); request.write_bool(true); request.write_i32(0);
        let status = system.call("query_domains", domain_api::SET_DOMAIN_VERIFICATION_USER_SELECTION,
            |out| out.write_raw(request.data(), request.objects()), domain_api::read_set_domain_verification_user_selection_reply).unwrap();
        assert_eq!(status, 3);
        let allocated = system.capture_package_queries().unwrap();
        assert_eq!(allocated.scan().version(), before_selection.scan().version() + 1);
        assert!(before_selection.domains().unwrap().owner().package("android").unwrap().users.is_empty());
        assert!(allocated.domains().unwrap().owner().package("android").unwrap().users[0].enabled_hosts.is_empty());
        assert_eq!(persistence.lock().unwrap().state().settings.domain_verification, saved_domains);
        assert_eq!(allocated.scan().owner().settings.domain_verification, before_selection.scan().owner().settings.domain_verification);
    }

#[test]
#[ignore = "requires pinned original inputs; host-only domain projection reproduction"]
fn native_domain_owner_publication_preserves_persistence_projection() {
    let oracle = |system: &Arc<System>, bridge: &Arc<crate::package::bootstrap::Bridge>,
        config: &SystemConfig, persistence: &Arc<Mutex<crate::package::owner::Store>>| {
        publish_fixture_domains(system, bridge, config, persistence, 4);
    };
    super::exercise_bootstrap_on(Driver::new(), true, false, Some(&oracle));
}

#[test]
#[ignore = "requires pinned image, built host/image, JDK and d8; run explicitly"]
fn original_art_reads_large_native_domain_query_over_binder() {
    let dir = std::env::temp_dir().join(format!("db-{}", std::process::id()));
    fs::create_dir(&dir).unwrap();
    let data = Data(dir);
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    let jdk = aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(java::sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            aim_paths::root()
                .join("crates/aim-services/tests/fixtures/NativeDomainBinderOracle.java"),
        ));
    run(
        Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/d8"))
            .args(["--min-api", "36", "--classpath"])
            .arg(&stubs)
            .arg("--output")
            .arg(&dex)
            .args(
                fs::read_dir(&classes)
                    .unwrap()
                    .map(|entry| entry.unwrap().path())
                    .filter(|path| path.extension().is_some_and(|ext| ext == "class")),
            ),
    );
    java::check_linkage(&dex.join("classes.dex"), &[]).unwrap();
    let name = format!("dev.aim.test.domain-info.{}", std::process::id());
    let server = aim_binder_host::server::Server::start(&name).unwrap();
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("g"));
    run(boot.start_command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let output = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "disposable domain boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let directory = boot.data.join("data/local/tmp/domain-binder");
    fs::create_dir(&directory).unwrap();
    fs::copy(dex.join("classes.dex"), directory.join("oracle.dex")).unwrap();
    let oracle = |system: &Arc<System>,
                  bridge: &Arc<crate::package::bootstrap::Bridge>,
                  config: &SystemConfig,
                  persistence: &Arc<Mutex<crate::package::owner::Store>>| {
        publish_fixture_domains(system, bridge, config, persistence, 4000);
        let process = system.process();
        super::register(&process, "query_domain_set", process.add_service(Arc::new(DomainSetEcho(Arc::downgrade(&process)))));
        for (alias, pattern) in [("query_uri_bounds", "["), ("query_uri_invalid", "*")] {
            use crate::package::{
                intent_filter::{IntentFilter, ParsedIntentInfo, UriRelativeFilterGroup},
                model::{PackageState, PackageUserState, State, User},
                pkg::{Activity, AndroidPackage, Component, MainComponent, booleans},
            };
            let mut filter = IntentFilter::default();
            filter.add_action("android.intent.action.VIEW");
            filter.add_data_scheme("https");
            filter.add_data_authority("x", None);
            let mut group = UriRelativeFilterGroup::new(0);
            group.add(0, 3, pattern);
            filter.add_uri_relative_filter_group(group);
            let pkg = AndroidPackage {
                package_name: "fixture.uri".into(),
                uid: 10001,
                booleans: booleans::ENABLED | booleans::HAS_CODE,
                activities: vec![Activity {
                    main: MainComponent {
                        component: Component {
                            name: "fixture.uri.View".into(),
                            package_name: "fixture.uri".into(),
                            intents: vec![ParsedIntentInfo {
                                filter,
                                ..ParsedIntentInfo::default()
                            }],
                            ..Component::default()
                        },
                        enabled: true,
                        exported: true,
                        ..MainComponent::default()
                    },
                    ..Activity::default()
                }],
                ..AndroidPackage::default()
            };
            let package = PackageState {
                name: "fixture.uri".into(),
                app_id: 10001,
                target_sdk_version: 35,
                pkg: Some(Arc::new(pkg)),
                users: [(0, PackageUserState::default())].into(),
                ..PackageState::default()
            };
            let state = State {
                packages: [("fixture.uri".into(), package)].into(),
                users: [(
                    0,
                    User {
                        unlocking_or_unlocked: true,
                        ..User::default()
                    },
                )]
                .into(),
                ..State::default()
            };
            let (service, _) = crate::package::service::PackageQueries::new(Arc::new(
                std::sync::RwLock::new(Arc::new(state)),
            ));
            super::register(&process, alias, process.add_service(service));
        }
        let output = boot.client_with_binder(1000, &name).args([ "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/domain-binder/oracle.dex", "/system/bin", "NativeDomainBinderOracle"])
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::GET_DOMAIN_VERIFICATION_INFO.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::DESCRIPTOR)
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::GET_DOMAIN_VERIFICATION_USER_STATE.to_string())
            .arg(system.capture_package_queries().unwrap().state().packages["android"].users[&0].domain_selection.as_ref().unwrap().0.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::GET_OWNERS_FOR_DOMAIN.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::SET_DOMAIN_VERIFICATION_LINK_HANDLING_ALLOWED.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::GET_URI_RELATIVE_FILTER_GROUPS.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::SET_DOMAIN_VERIFICATION_STATUS.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::SET_DOMAIN_VERIFICATION_USER_SELECTION.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::SET_URI_RELATIVE_FILTER_GROUPS.to_string())
            .arg(aim_service_aidl::android_content_pm_ipackagemanager::QUERY_INTENT_ACTIVITIES.to_string())
            .output().unwrap();
        if !output.status.success() {
            let logs = boot
                .command()
                .args(["shell", "logcat", "-d", "-s", "AndroidRuntime:V"])
                .output()
                .unwrap();
            panic!(
                "original domain Binder oracle: {}; stdout: {}; stderr: {}; AndroidRuntime: {}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                String::from_utf8_lossy(&logs.stdout)
            );
        }
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            "NATIVE_DOMAIN_BINDER 4000\n"
        );
    };
    super::exercise_bootstrap_on(server.driver().clone(), true, true, Some(&oracle));
}
