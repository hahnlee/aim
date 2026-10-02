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
