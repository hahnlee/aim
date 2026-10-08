//! Original ComponentResolver declaration mutation versus native accepted registry.
use super::*;
#[allow(dead_code)]
#[path = "../../tests/common/java.rs"] mod java;
#[path = "../../tests/common/runtime.rs"] mod runtime;
use runtime::{Boot,Data,run};
use std::{fs,process::Command,time::{Duration,Instant}};
#[test]
#[ignore = "requires pinned image, built host/image, JDK and d8; run explicitly"]
fn original_provider_registration_matches_native_runtime_views() {
    let dir = std::env::temp_dir().join(format!("aim-prov-{}", std::process::id()));
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
                .join("crates/aim-services/tests/fixtures/ProviderRegistrationOracle.java"),
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
    java::check_linkage(&dex.join("classes.dex"), &["/system/framework/services.jar"]).unwrap();
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
            "disposable keyset boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let directory = boot.data.join("data/local/tmp/provider-registration");
    fs::create_dir(&directory).unwrap();
    fs::copy(dex.join("classes.dex"), directory.join("oracle.dex")).unwrap();
    use crate::package::{pkg::{AndroidPackage,Provider,MainComponent,Component},sign::SigningDetails};
    let provider=|package:&str,name:&str,authorities:&str,syncable:bool| Provider {
        main:MainComponent {component:Component {name:name.into(),package_name:package.into(),properties:Some(vec![]),..Default::default()},..Default::default()},authority:Some(authorities.into()),syncable,uri_permission_patterns:Some(vec![]),path_permissions:Some(vec![]),..Default::default()
    };
    let packages=[
        AndroidPackage {feature_flag_state:Some(vec![]),package_name:"fixture.first".into(),providers:vec![provider("fixture.first","BB","one",false)],..Default::default()},
        AndroidPackage {feature_flag_state:Some(vec![]),package_name:"fixture.sync".into(),providers:vec![provider("fixture.sync","BB","one;two;three;",true),provider("fixture.sync","Aa","four;five",false)],..Default::default()},
        AndroidPackage {feature_flag_state:Some(vec![]),package_name:"fixture.sync".into(),providers:vec![provider("fixture.sync","Aa","two;six;seven",true),provider("fixture.sync","BB","one;four",false)],..Default::default()},
        AndroidPackage {feature_flag_state:Some(vec![]),package_name:"fixture.sync".into(),providers:vec![provider("fixture.sync","BB","one;two;three",false),provider("fixture.sync","Aa","eight;nine",true)],..Default::default()},
    ];
    let mut registry=Registry::default();let mut expected=Vec::new();
    for (i,package) in packages.iter().enumerate() {
        let raw=package.to_cache_entry().unwrap().bytes;fs::write(directory.join(format!("raw-{i}")),&raw).unwrap();
        registry.register(Arc::new(LoadedPackage::new(package.clone(),SigningDetails::unknown()).unwrap())).unwrap();
        let runtime=registry.package_view(&package.package_name,package).unwrap();
        assert_eq!(package.to_cache_entry().unwrap().bytes,raw,"native registration mutated raw code");
        expected.push(runtime);
    }
    let output=boot.client(1000).args(["/system/bin/app_process","-Djava.class.path=/data/local/tmp/provider-registration/oracle.dex:/system/framework/services.jar","/system/bin","ProviderRegistrationOracle","/data/local/tmp/provider-registration"]).output().unwrap();
    assert!(output.status.success(),"original provider registration: {} {}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
    for (i,native) in expected.iter().enumerate() {
        let original=AndroidPackage::read_cache_entry(&fs::read(directory.join(format!("original-{i}"))).unwrap()).unwrap();
        assert_eq!(original.providers,native.providers,"original registered declaration provider {i}");
    }
}
