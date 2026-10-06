//! Original ART → native DomainQueries → FD-backed Info → original creator.
use super::*;
#[path = "../../tests/common/java.rs"]
mod java;
#[path = "../../tests/common/runtime.rs"]
mod runtime;
use runtime::{Boot, Data, run};
use std::{fs, process::Command};

#[test]
#[ignore = "requires pinned image, built host/image, JDK and d8; run explicitly"]
fn original_art_reads_large_native_domain_query_over_binder() {
    let dir = std::env::temp_dir().join(format!("aim-domain-binder-{}", std::process::id()));
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
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
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
        let base = system.capture_package_scan().unwrap();
        let mut candidate = base.owner().clone();
        // Only fixture metadata changes; its original APK and extracted tree stay read-only.
        candidate.add_fixture_domains("android", 4000);
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
        let context = super::query_context_for(&candidate, base.version() + 1);
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
        let output = boot.client(1000).args(["--binder", &name, "/system/bin/app_process",
            "-Djava.class.path=/data/local/tmp/domain-binder/oracle.dex", "/system/bin", "NativeDomainBinderOracle"])
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::GET_DOMAIN_VERIFICATION_INFO.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::DESCRIPTOR)
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::GET_DOMAIN_VERIFICATION_USER_STATE.to_string())
            .arg(system.capture_package_queries().unwrap().state().packages["android"].users[&0].domain_selection.as_ref().unwrap().0.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::GET_OWNERS_FOR_DOMAIN.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::SET_DOMAIN_VERIFICATION_LINK_HANDLING_ALLOWED.to_string())
            .arg(aim_service_aidl::android_content_pm_verify_domain_idomainverificationmanager::GET_URI_RELATIVE_FILTER_GROUPS.to_string())
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
