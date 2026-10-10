//! Verify the compiled policy in the actual pinned image, not an absent
//! aconfig flag's default. Missing inputs are a failed explicit run.
use aim_android_image::dex::{Dex, instruction_units, units};
use aim_apps::apk::Apk;
use aim_services::package::libraries::Policy;

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn sdk_dependency_collection_requires_the_library_in_the_pinned_image() {
    let jar =
        Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
    let mut calls = 0;
    for name in ["classes.dex", "classes2.dex", "classes3.dex"] {
        let bytes = jar.file(name).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        let Some(class) = dex.class("Lcom/android/server/pm/SharedLibrariesImpl;") else {
            continue;
        };
        for code in dex
            .methods_named(class, "collectSharedLibraryInfos")
            .unwrap()
        {
            let words = units(&bytes, &code).unwrap();
            let mut sdk = false;
            let mut constants = std::collections::BTreeMap::new();
            let mut at = 0;
            while at < words.len() {
                let opcode = words[at] & 255;
                let size = instruction_units(&words, at).unwrap();
                if opcode == 0x12 {
                    // const/4: low register nibble, high signed literal nibble.
                    let register = (words[at] >> 8) & 15;
                    let value = (words[at] as i16) >> 12;
                    constants.insert(register, (value, at));
                }
                if matches!(opcode, 0x6e..=0x72 | 0x74..=0x78) {
                    let (owner, method, signature) = dex.method(words[at + 1] as u32).unwrap();
                    if method == "getUsesSdkLibraries" {
                        sdk = true;
                        constants.clear();
                    }
                    if sdk && owner == class.descriptor && method == "collectSharedLibraryInfos" {
                        assert_eq!(
                            signature,
                            "(Ljava/util/List;[J[[Ljava/lang/String;[ZLjava/lang/String;Ljava/lang/String;ZILjava/util/ArrayList;Ljava/util/Map;Ljava/util/Map;Ljava/util/List;)Ljava/util/ArrayList;"
                        );
                        assert!(
                            matches!(opcode, 0x74..=0x78),
                            "expected range invoke in pinned SDK call"
                        );
                        assert_eq!(words[at] >> 8, 13);
                        let required_register = words[at + 2] + 7;
                        let (value, assigned) = constants[&required_register];
                        assert_eq!(value, 1, "SDK independence policy changed in the image");
                        // In this compiled call block only argument moves and
                        // string constants follow required=true. Reject a new
                        // branch/call or any reassignment of that argument.
                        let mut next = assigned + 1;
                        while next < at {
                            let op = words[next] & 255;
                            let destination = match op {
                                0x07 => (words[next] >> 8) & 15,
                                0x08 | 0x1a | 0x1b => words[next] >> 8,
                                _ => panic!("SDK call block changed: opcode {op:#x}"),
                            };
                            assert_ne!(destination, required_register);
                            next += instruction_units(&words, next).unwrap();
                        }
                        calls += 1;
                        sdk = false;
                    }
                }
                at += size;
            }
        }
    }
    assert_eq!(
        calls, 1,
        "expected the pinned SDK dependency collection call"
    );
    assert!(!Policy::pinned(true).sdk_library_independence);
}

#[test]
#[ignore = "requires the pinned original image; run explicitly"]
fn sdk_libraries_require_an_app_id_in_the_pinned_settings_owner() {
    use sha2::{Digest, Sha256};
    let jar =
        Apk::open(&aim_paths::original_image().join("system/framework/services.jar")).unwrap();
    let mut checked = 0;
    for name in ["classes.dex", "classes2.dex", "classes3.dex"] {
        let bytes = jar.file(name).unwrap();
        let dex = Dex::parse(&bytes).unwrap();
        let Some(class) = dex.class("Lcom/android/server/pm/Settings;") else {
            continue;
        };
        for (name, digest) in [
            (
                "addPackageLPw",
                "58647c19cc00db53606912ef2564148b12770c9d82070576bf226f5a28fe3aae",
            ),
            (
                "readPackageLPw",
                "f0d10b6622b22c5b17d769ca67c1e8f65e5be0ecb2a642a5769f360b0539abb4",
            ),
        ] {
            let methods = dex.methods_named(class, name).unwrap();
            assert_eq!(methods.len(), 1);
            let words = units(&bytes, &methods[0]).unwrap();
            // Fingerprint the complete inspected control flow: a future
            // image must re-establish policy, including register assignment
            // and branch targets, rather than infer an absent flag's default.
            let data: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            assert_eq!(
                format!("{:x}", Sha256::digest(data)),
                digest,
                "{name} changed"
            );
            if name == "addPackageLPw" {
                // setAppId -> result -> mAppIds -> registerExistingAppId,
                // with no SDK/no-ID bypass. The SDK argument is unused.
                assert_eq!(dex.method(words[0x35] as u32).unwrap().1, "setAppId");
                assert_eq!(words[0x37], 0x0b0c);
                assert_eq!(words[0x38] & 255, 0x54);
                assert_eq!(words[0x3a], 0x406e);
                assert_eq!(
                    dex.method(words[0x3b] as u32).unwrap().1,
                    "registerExistingAppId"
                );
            } else {
                assert_eq!(dex.method(words[0x1d] as u32).unwrap().1, "parseAppId");
                assert_eq!(words[0x1f], 0x050a); // result v5
                assert_eq!(&words[0x9f..0xa1], &[0x2202, 5]); // v34 = v5
                // Only appId > 0 takes the standalone addPackage path.
                assert_eq!(&words[0x277..0x27b], &[0x223c, 0xf7, 0x0738, 0xa2]);
                assert_eq!(0x277 + words[0x278] as usize, 0x36e);
                assert_eq!(dex.method(words[0x3ab] as u32).unwrap().1, "addPackageLPw");
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 2);
}

#[test]
fn settings_keep_sdk_libraries_only_with_valid_id_ownership() {
    use aim_services::package::{
        owner::app_ids::{AppIds, Owner},
        settings::Settings,
    };
    let xml = br#"<packages>
        <shared-user name="group" userId="1000" />
        <package name="sdk-no-id" codePath="/data/app/no-id" userId="-1" isSdkLibrary="true" />
        <package name="sdk-zero-id" codePath="/data/app/zero" userId="0" isSdkLibrary="true" />
        <package name="app-no-id" codePath="/data/app/no-id-app" userId="-1" />
        <package name="sdk-id" codePath="/data/app/sdk" userId="10001" isSdkLibrary="true" />
        <package name="sdk-shared" codePath="/data/app/shared" userId="-1" sharedUserId="1000" isSdkLibrary="true" />
    </packages>"#;
    let settings = Settings::parse(&aim_android_xml::read(xml).unwrap()).unwrap();
    assert_eq!(
        settings
            .packages
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["sdk-id", "sdk-shared"]
    );
    assert!(settings.packages.iter().all(|p| p.is_sdk_library));
    let ids = AppIds::restore(&settings).unwrap();
    assert_eq!(ids.get(10001), Some(&Owner::Package("sdk-id".into())));
    assert_eq!(ids.get(1000), Some(&Owner::SharedUser("group".into())));
    assert_eq!(ids.get(-1), None);
}
