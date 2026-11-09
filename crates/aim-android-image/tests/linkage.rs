//! Compile-only stubs are checked against the pinned image's classes,
//! their members' access included (#1035).

mod common;

use aim_android_image::classpath::{self, BOOTCLASSPATH};
use aim_android_image::linkage::ClassPath;
use std::path::Path;

const FRAMEWORK: &str = "system/framework/framework.jar";

#[test]
fn stubs_wider_than_the_image_are_mismatches() {
    let Some(image) = aim_paths::original_image_with(FRAMEWORK) else {
        return;
    };
    let Some((jdk, tools)) = common::java_toolchain() else {
        return;
    };
    let jars = classpath::jars(&image, "bootclasspath.pb", BOOTCLASSPATH).unwrap();
    let classpath = ClassPath::read(&image, &jars).unwrap();
    let work = std::env::temp_dir().join(format!("linkage-{}", std::process::id()));
    let mismatches = |stub: &str| {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/linkage")
            .join(stub)
            .join("android/graphics/Bitmap.java");
        let dex = common::dex(&jdk, &tools, &[source], &work.join(stub));
        classpath.stub_mismatches(&dex).unwrap()
    };
    let wide = mismatches("wide");
    let exact = mismatches("exact");
    std::fs::remove_dir_all(&work).unwrap();
    let method = "method Landroid/graphics/Bitmap;->getDefaultDensity()I";
    assert!(
        wide.contains(&format!("{method} has access 0x9, the image's 0x8")),
        "{wide:?}"
    );
    // Both stubs have Java's default constructor, which the image lacks.
    assert_eq!(exact, ["no method Landroid/graphics/Bitmap;-><init>()V"]);
    assert_eq!(wide.len(), 2, "{wide:?}");
}
