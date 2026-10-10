//! Original ART KeySet capability roundtrip through retained native query leases.
use super::*;
#[allow(dead_code)]
#[path = "../../tests/common/java.rs"] mod java;
#[path = "../../tests/common/runtime.rs"] mod runtime;
use runtime::{Boot,Data,run};
use std::{fs,process::Command};
#[test]
#[ignore = "requires pinned image, built host/image, JDK and d8; run explicitly"]
fn original_art_keysets_roundtrip_native_query_leases() {
    let dir = std::env::temp_dir().join(format!("kb-{}", std::process::id()));
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
                .join("crates/aim-services/tests/fixtures/KeySetBinderOracle.java"),
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
    let name = format!("dev.aim.test.keysets.{}", std::process::id());
    let server = aim_binder_host::server::Server::start(&name).unwrap();
    let boot = Boot::new(aim_paths::root().join("target/release/aimctl"), data.0.join("g"));
    run(boot.start_command().args(["start", "--windows"]));
    boot.wait_ready(Duration::from_secs(300)).expect("keyset_transport_test original oracle boot readiness");
    let directory = boot.data.join("data/local/tmp/keyset-binder");
    fs::create_dir(&directory).unwrap();
    fs::copy(dex.join("classes.dex"), directory.join("oracle.dex")).unwrap();
    let oracle=|system: &Arc<System>, _bridge: &Arc<crate::package::bootstrap::Bridge>, _config: &SystemConfig, _store: &Arc<Mutex<crate::package::owner::Store>>| {
        let process=system.process();
        let capture=system.capture_package_queries().unwrap();
        let capture=if capture.state().system.key_set_tokens.is_some() {capture} else {capture.prepare_keysets(process.clone()).unwrap()};
        let context=super::query_context_for(capture.scan().owner(),capture.scan().version());
        let replacement=crate::package::scan_snapshot::query_state::Capture::new(capture.scan().clone(),context).unwrap().prepare_keysets(process.clone()).unwrap();
        for (name,owner) in [("keyset_computer",capture),("keyset_replacement",replacement)] {
            let computer=crate::package::scan_snapshot::computer::Computer::new(owner,Arc::downgrade(&process));
            super::register(&process,name,process.add_service(Arc::new(computer)));
        }
        use aim_service_aidl::dev_aim_server_ipackagecomputer as api;
        let output=boot.client_with_binder(1000, &name).args(["/system/bin/app_process","-Djava.class.path=/data/local/tmp/keyset-binder/oracle.dex","/system/bin","KeySetBinderOracle"])
            .arg(api::GET_PACKAGE_MANAGER_QUERY_BINDER.to_string()).arg(api::DESCRIPTOR).arg(api::CLOSE.to_string()).output().unwrap();
        assert!(output.status.success(),"original KeySet Binder oracle: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        assert_eq!(String::from_utf8(output.stdout).unwrap(),"ORIGINAL_KEYSET_BINDER stable subset exact null foreign owners close\n");
    };
    super::exercise_bootstrap_on(server.driver().clone(),true,true,Some(&oracle));
}
