//! FallbackCategoryProvider creates a bare framework AssetManager instead
//! of using the system context's overlaid Resources.

use aim_apps::apk::Apk;
use aim_services::package::{parse::Platform, system_config};

#[test]
#[ignore = "requires the pinned derived image"]
fn fallback_categories_use_the_bare_framework_resource() {
    let image = aim_paths::derived_image();
    let platform = Platform::load(&image, Default::default()).unwrap();
    let expected = Apk::open(&image.join("system/framework/framework-res.apk"))
        .unwrap()
        .file("res/raw/fallback_categories.csv")
        .unwrap();
    assert_eq!(
        platform
            .framework_file_without_overlays(&image, "raw", "fallback_categories")
            .unwrap(),
        expected,
    );
    let framework = system_config::Framework::load(&image).unwrap();
    let system = system_config::system(&image, &|_| None, &framework);
    assert_eq!(
        system.fallback_categories,
        [("com.android.printspooler".into(), 7)],
    );
}
