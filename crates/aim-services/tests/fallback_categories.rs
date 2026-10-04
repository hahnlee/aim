//! FallbackCategoryProvider's newly created AssetManager includes the
//! pinned image's system assets and their static overlays.

use aim_apps::apk::Apk;
use aim_services::package::{parse::Platform, system_config};

#[test]
#[ignore = "requires the pinned derived image"]
fn fallback_categories_use_original_system_assets() {
    let image = aim_paths::derived_image();
    let platform = Platform::load(&image, Default::default()).unwrap();
    let expected = Apk::open(&image.join("product/overlay/GoogleConfigOverlay.apk"))
        .unwrap()
        .file("res/raw/fallback_categories.csv")
        .unwrap();
    assert_eq!(
        platform
            .framework_file(&image, "raw", "fallback_categories")
            .unwrap(),
        expected,
    );
    let framework = system_config::Framework::load(&image).unwrap();
    let system = system_config::system(&image, &|_| None, &framework).unwrap();
    assert_eq!(system.fallback_categories.len(), 387);
    assert!(
        !system
            .fallback_categories
            .iter()
            .any(|(p, _)| p == "com.android.printspooler")
    );
}
