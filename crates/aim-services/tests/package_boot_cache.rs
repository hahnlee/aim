//! Host-only PackageCacher/collectCertificatesLI checks over the pinned image.
use aim_android_init::ImageRoot;
use aim_services::package::{
    parse::{Platform, parcel::java_hash},
    scan::{Apex, Code, Image, Partition},
    settings::{Package, Signatures},
    write::{Apks, CertificateCollection},
};
use std::{
    fs::{self, File, FileTimes},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("aim-package-cache-{name}-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
    fn cache(&self) -> PathBuf {
        self.0.join("data/system/package_cache/fingerprint")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn apks(root: &Path, identity: PathBuf) -> Apks {
    let features = aim_services::package::system_config::SystemConfig::read(root, &|_| None)
        .features
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    let mut platform = Platform::load(root, features).unwrap();
    // A controlled host display, since the emulator image sets density at boot.
    platform.density_dpi = Some(320);
    let root = ImageRoot::new(root.to_owned());
    Apks {
        platform,
        signing_overrides: None,
        files: Box::new(move |path| {
            Some(if path == "/.identity" {
                identity.clone()
            } else {
                root.host_path(path)
            })
        }),
    }
}

fn apexes(root: &Path) -> Vec<Apex> {
    let mut names: Vec<_> = fs::read_dir(root.join("apex"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| Apex {
            module_name: Some(name.clone()),
            mount_path: format!("/apex/{name}"),
            partition: Partition::System,
            factory: true,
            active_changed: false,
        })
        .collect()
}

fn equal(a: &Image<()>, b: &Image<()>) {
    assert_eq!(a.rejected, b.rejected);
    assert_eq!(a.packages.len(), b.packages.len());
    for (a, b) in a.packages.iter().zip(&b.packages) {
        assert_eq!(a.location, b.location);
        assert_eq!(a.parsed, b.parsed);
    }
}

fn receipts(cache: &Path) -> Vec<(PathBuf, u64)> {
    let mut files: Vec<_> = fs::read_dir(cache)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.path(), e.metadata().unwrap().ino())
        })
        .collect();
    files.sort();
    files
}

fn collect(apks: &Apks, image: &Image<()>, saved: &[Package]) -> Vec<Code> {
    image
        .packages
        .iter()
        .map(|code| {
            let signing = apks
                .collect_signing_details(
                    &code.parsed,
                    CertificateCollection {
                        saved: saved
                            .iter()
                            .find(|p| Some(p.code_path.as_str()) == code.parsed.path.as_deref()),
                        database_version: 3,
                        force_collect: false,
                        skip_verify: true,
                        pre_n_mr1_upgrade: false,
                    },
                )
                .unwrap();
            Code {
                location: code.location.clone(),
                parsed: code.parsed.clone(),
                signing,
            }
        })
        .collect()
}

fn saved(apks: &Apks, codes: &[Code]) -> Vec<Package> {
    codes
        .iter()
        .map(|code| Package {
            name: code.parsed.package_name.clone(),
            code_path: code.parsed.path.clone().unwrap(),
            last_modified_time: apks.scan_file_time(&code.parsed).unwrap(),
            signatures: Some(Signatures {
                scheme_version: code.signing.scheme_version,
                signatures: code.signing.signatures.clone(),
                current_flags: code.signing.current_flags.clone(),
                public_keys: code.signing.serialized_public_keys().unwrap(),
                past_signatures: code.signing.past_signing_certificates.clone(),
            }),
            ..Default::default()
        })
        .collect()
}

#[test]
#[ignore = "requires pinned image; host-only, no guest boot"]
fn cached_second_scan_equals_cold_image_scan() {
    let original = aim_paths::original_image();
    let fixture = Fixture::new("parity");
    let apks = apks(&original, original.with_extension("identity"));
    let apexes = apexes(&original);
    let first = Image::parse_cached(&apks, &apexes, Some(&fixture.cache())).unwrap();
    let before = receipts(&fixture.cache());
    assert_eq!(before.len(), first.packages.len() * 2);
    let first_signing = collect(&apks, &first, &[]);
    // A second parser and a new image scan, sharing only disk cache/settings.
    let second_apks = self::apks(&original, original.with_extension("identity"));
    let warm = Image::parse_cached(&second_apks, &apexes, Some(&fixture.cache())).unwrap();
    assert_eq!(
        receipts(&fixture.cache()),
        before,
        "cache hits must not rewrite entries"
    );
    let cold = Image::parse(&second_apks, &apexes).unwrap();
    equal(&first, &warm);
    equal(&cold, &warm);
    let settings = saved(&apks, &first_signing);
    let warm_signing = collect(&second_apks, &warm, &settings);
    let cold_signing = collect(&second_apks, &cold, &[]);
    for (a, b) in warm_signing.iter().zip(&cold_signing) {
        assert_eq!(a.signing, b.signing);
    }
    println!(
        "{} packages, {} rejections: cached/cold parcels, order and signing identical",
        warm.packages.len(),
        warm.rejected.len()
    );
}

#[test]
#[ignore = "requires pinned image; host-only, modifies only disposable copies"]
fn changed_apk_and_configuration_invalidate_cached_parse() {
    let original = aim_paths::original_image();
    let fixture = Fixture::new("invalidate");
    let framework = fixture.0.join("system/framework");
    let app = fixture.0.join("product/app/Test");
    fs::create_dir_all(&framework).unwrap();
    fs::create_dir_all(&app).unwrap();
    fs::copy(
        original.join("system/framework/framework-res.apk"),
        framework.join("framework-res.apk"),
    )
    .unwrap();
    let source =
        original.join("system_ext/priv-app/GoogleServicesFramework/GoogleServicesFramework.apk");
    let target = app.join("base.apk");
    fs::copy(&source, &target).unwrap();
    let identity = fixture.0.join(".identity");
    fs::write(&identity, b"image-one").unwrap();
    let mut apks = apks(&original, identity.clone());
    let root = ImageRoot::new(fixture.0.clone());
    apks.files = Box::new(move |path| Some(root.host_path(path)));
    let cache = fixture.cache();
    let cold = Image::parse(&apks, &[]).unwrap();
    assert!(!cache.exists(), "a disabled cache must not create files");
    let blocked = fixture.0.join("not-a-cache-directory");
    fs::write(&blocked, b"not a directory").unwrap();
    equal(
        &cold,
        &Image::parse_cached(&apks, &[], Some(&blocked)).unwrap(),
    );
    let first = Image::parse_cached(&apks, &[], Some(&cache)).unwrap();
    equal(&cold, &first);
    // Replace a copied APK by another valid APK; the original tree is read-only.
    fs::copy(original.join("system/framework/framework-res.apk"), &target).unwrap();
    let changed = Image::parse_cached(&apks, &[], Some(&cache)).unwrap();
    equal(&changed, &Image::parse(&apks, &[]).unwrap());
    assert_ne!(
        first.packages.last().unwrap().parsed,
        changed.packages.last().unwrap().parsed
    );
    let old = receipts(&cache);
    fs::write(&identity, b"image-two").unwrap();
    equal(
        &changed,
        &Image::parse_cached(&apks, &[], Some(&cache)).unwrap(),
    );
    assert_ne!(old, receipts(&cache));
    let old = receipts(&cache);
    apks.platform.density_dpi = Some(640);
    equal(
        &Image::parse_cached(&apks, &[], Some(&cache)).unwrap(),
        &Image::parse(&apks, &[]).unwrap(),
    );
    assert_ne!(old, receipts(&cache));
    let entry = cache.join(format!(
        "framework-res.apk-16-{}",
        java_hash("/system/framework/framework-res.apk")
    ));
    fs::write(&entry, b"torn parcel").unwrap();
    equal(
        &Image::parse_cached(&apks, &[], Some(&cache)).unwrap(),
        &Image::parse(&apks, &[]).unwrap(),
    );
}

#[test]
#[ignore = "requires pinned image; host-only certificate collection gates"]
fn unchanged_signing_uses_saved_details_but_changed_or_forced_code_is_recollected() {
    let original = aim_paths::original_image();
    let fixture = Fixture::new("signing");
    let target = fixture.0.join("base.apk");
    fs::copy(original.join("system/framework/framework-res.apk"), &target).unwrap();
    let mut apks = apks(&original, original.with_extension("identity"));
    let host = target.clone();
    apks.files = Box::new(move |_| Some(host.clone()));
    let parsed = apks.parsed_path("/system/app/Test.apk", 16).unwrap();
    let image = Image {
        packages: vec![Code {
            location: aim_services::package::scan::Location {
                path: parsed.path.clone().unwrap(),
                partition: Partition::System,
                kind: aim_services::package::scan::Kind::App,
                apex: None,
            },
            parsed,
            signing: (),
        }],
        rejected: vec![],
    };
    let first = collect(&apks, &image, &[]);
    let settings = saved(&apks, &first);
    let time = fs::metadata(&target).unwrap().modified().unwrap();
    fs::write(&target, b"invalid APK: opening this for signing must fail").unwrap();
    File::open(&target)
        .unwrap()
        .set_times(FileTimes::new().set_modified(time))
        .unwrap();
    let choice = CertificateCollection {
        saved: Some(&settings[0]),
        database_version: 3,
        force_collect: false,
        skip_verify: true,
        pre_n_mr1_upgrade: false,
    };
    let pkg = &image.packages[0].parsed;
    assert_eq!(
        apks.collect_signing_details(pkg, choice).unwrap(),
        first[0].signing
    );
    for choice in [
        CertificateCollection {
            force_collect: true,
            ..choice
        },
        CertificateCollection {
            database_version: 2,
            ..choice
        },
    ] {
        assert!(apks.collect_signing_details(pkg, choice).is_err());
    }
    File::open(&target)
        .unwrap()
        .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1)))
        .unwrap();
    assert!(apks.collect_signing_details(pkg, choice).is_err());
}

#[test]
#[ignore = "host benchmark over pinned image; run with --release --ignored --nocapture"]
fn benchmark_raw_scan_and_signing() {
    let original = aim_paths::original_image();
    let fixture = Fixture::new("bench");
    let apks = apks(&original, original.with_extension("identity"));
    let apexes = apexes(&original);
    for run in 1..=5 {
        if fixture.cache().exists() {
            fs::remove_dir_all(fixture.cache()).unwrap();
        }
        let start = Instant::now();
        let cold = Image::parse_cached(&apks, &apexes, Some(&fixture.cache())).unwrap();
        let parse_cold = start.elapsed();
        let codes = collect(&apks, &cold, &[]);
        let cold_total = start.elapsed();
        let saved = saved(&apks, &codes);
        let start = Instant::now();
        let warm = Image::parse_cached(&apks, &apexes, Some(&fixture.cache())).unwrap();
        let parse_warm = start.elapsed();
        let warm_codes = collect(&apks, &warm, &saved);
        let warm_total = start.elapsed();
        equal(&cold, &warm);
        for (a, b) in codes.iter().zip(warm_codes) {
            assert_eq!(a.signing, b.signing);
        }
        println!(
            "run={run} packages={} cold_parse_ms={:.3} cold_sign_ms={:.3} cold_total_ms={:.3} warm_parse_ms={:.3} warm_sign_ms={:.3} warm_total_ms={:.3}",
            cold.packages.len(),
            parse_cold.as_secs_f64() * 1000.,
            (cold_total - parse_cold).as_secs_f64() * 1000.,
            cold_total.as_secs_f64() * 1000.,
            parse_warm.as_secs_f64() * 1000.,
            (warm_total - parse_warm).as_secs_f64() * 1000.,
            warm_total.as_secs_f64() * 1000.
        );
    }
}
