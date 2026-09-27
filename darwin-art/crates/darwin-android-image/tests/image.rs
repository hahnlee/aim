//! Synthetic-tree tests for the overlay manifest, assembly, identity and diff.

use darwin_android_image::assemble::force_remove;
use darwin_android_image::diff::{self, Header};
use darwin_android_image::identity::{self, Identity};
use darwin_android_image::manifest::parse;
use darwin_android_image::{Outcome, Plan, ProblemKind, assemble, validate};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

const ORIGINAL_ID: &str = "1111111111111111111111111111111111111111111111111111111111111111";

struct Temp(PathBuf);

impl Temp {
    fn new() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "android-image-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = force_remove(&path);
        fs::create_dir_all(&path).unwrap();
        Temp(path)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = force_remove(&self.0);
    }
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

/// A small original tree (read-only, as an extraction leaves it) and a
/// source root.
struct Fixture {
    temp: Temp,
    original: PathBuf,
    sources: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let temp = Temp::new();
        let original = temp.0.join("original");
        let sources = temp.0.join("repo");
        write(&original.join(".identity"), &format!("{ORIGINAL_ID}\n"));
        write(&original.join("system/etc/hosts"), "127.0.0.1 localhost\n");
        write(&original.join("system/bin/toybox"), "#!elf\n");
        fs::set_permissions(
            original.join("system/bin/toybox"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        write(&original.join("system/app/Stock/Stock.apk"), "apk-bytes");
        write(
            &original.join("system/app/Stock/oat/arm64/Stock.odex"),
            "odex",
        );
        write(&original.join("system/vendor-real/placeholder"), "");
        symlink("/system/vendor-real", original.join("system/vendor")).unwrap();
        fs::create_dir_all(original.join("vendor/etc")).unwrap();
        write(
            &sources.join("image/files/hosts"),
            "127.0.0.1 localhost\n::1 localhost\n",
        );
        write(&sources.join("_build/vendor/manifest.xml"), "<manifest/>\n");
        let fixture = Fixture {
            temp,
            original,
            sources,
        };
        seal_tree(&fixture.original);
        fixture
    }

    fn plan(&self, text: &str) -> Plan {
        let manifest = parse(text).expect("parses");
        validate(&manifest, &self.original, &self.sources).expect("validates")
    }

    fn problems(&self, text: &str) -> Vec<ProblemKind> {
        let manifest = match parse(text) {
            Ok(manifest) => manifest,
            Err(problems) => return problems.iter().map(|p| p.kind).collect(),
        };
        match validate(&manifest, &self.original, &self.sources) {
            Ok(_) => Vec::new(),
            Err(problems) => problems.iter().map(|p| p.kind).collect(),
        }
    }
}

/// Makes the original read-only the way an extraction would.
fn seal_tree(root: &Path) {
    let mut directories = vec![root.to_path_buf()];
    let mut index = 0;
    while index < directories.len() {
        for entry in fs::read_dir(&directories[index]).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                directories.push(entry.path());
            }
        }
        index += 1;
    }
    for directory in directories.iter().rev() {
        fs::set_permissions(directory, fs::Permissions::from_mode(0o555)).unwrap();
    }
}

const OVERLAY: &str = r#"
schema = 1

[[replace]]
path = "/system/etc/hosts"
source = "image/files/hosts"
reason = "IPv6 loopback"

[[add]]
path = "/vendor/etc/vintf/manifest.xml"
source = "_build/vendor/manifest.xml"

[[remove]]
path = "/system/app/Stock"
reason = "no such hardware"
"#;

fn compute(fixture: &Fixture, text: &str) -> Identity {
    identity::compute(ORIGINAL_ID, &fixture.plan(text))
}

#[test]
fn checked_in_manifest_parses() {
    let text = include_str!("../../../image/overlay.toml");
    parse(text).expect("the checked-in manifest parses");
}

#[test]
fn parse_reports_syntax_and_shape_errors() {
    let text = r#"
[[add]]
path = "/vendor/a"
source = "x"
color = "blue"

[[replace]]
path = "/system/etc/hosts"
source = "y"

[[remove]]
path = "/system/app"
source = "z"
reason = "  "

[system]
key = "unterminated
"#;
    let problems = parse(text).unwrap_err();
    let kinds: Vec<_> = problems.iter().map(|p| p.kind).collect();
    for expected in [
        ProblemKind::Schema,
        ProblemKind::Syntax,
        ProblemKind::UnexpectedField,
        ProblemKind::MissingReason,
    ] {
        assert!(
            kinds.contains(&expected),
            "{expected:?} missing from {problems:#?}"
        );
    }
    // color, [system], unterminated string, missing replace reason,
    // remove source, empty remove reason, missing schema.
    assert_eq!(problems.len(), 7, "{problems:#?}");
    assert!(parse("schema = 2\n").is_err());
    assert!(parse("schema = 1\n[[add]]\npath = '''x'''\n").is_err());
}

#[test]
fn parse_accepts_escapes_literals_and_comments() {
    let manifest = parse(
        "schema = 1 # v1\n[[add]] # new\npath = '/vendor/lit' # c\nsource = \"a\\u0062\"\nreason = \"tab\\tquote\\\"\"\n",
    )
    .unwrap();
    let entry = &manifest.entries[0];
    assert_eq!(entry.path, "/vendor/lit");
    assert_eq!(entry.source.as_deref(), Some("ab"));
    assert_eq!(entry.reason.as_deref(), Some("tab\tquote\""));
}

#[test]
fn validation_rejects_bad_paths_and_sources() {
    let fixture = Fixture::new();
    let add = |path: &str, source: &str| {
        format!("schema = 1\n[[add]]\npath = \"{path}\"\nsource = \"{source}\"\n")
    };
    let manifest_xml = "_build/vendor/manifest.xml";
    for path in [
        "vendor/x",
        "/vendor/../x",
        "/vendor/./x",
        "/vendor//x",
        "/vendor/x/",
        "/",
    ] {
        assert_eq!(
            fixture.problems(&add(path, manifest_xml)),
            [ProblemKind::InvalidGuestPath],
            "{path}"
        );
    }
    for source in [
        "../outside",
        "/etc/passwd",
        "image/../_build/vendor/manifest.xml",
        "./x",
        "",
    ] {
        assert_eq!(
            fixture.problems(&add("/vendor/x", source)),
            [ProblemKind::InvalidSourcePath],
            "{source}"
        );
    }
    assert_eq!(
        fixture.problems(&add("/vendor/x", "_build/missing")),
        [ProblemKind::SourceMissing]
    );
    assert_eq!(
        fixture.problems(&add("/vendor/x", "_build/vendor")),
        [ProblemKind::SourceMissing],
        "a directory is not a source file"
    );
    assert_eq!(
        fixture.problems(&add("/system/vendor/x", manifest_xml)),
        [ProblemKind::Traversal],
        "a symlink in the original could lead outside the image"
    );
    assert_eq!(
        fixture.problems(&add("/system/etc/hosts/x", manifest_xml)),
        [ProblemKind::NotADirectory]
    );
    assert_eq!(
        fixture.problems(&add("/.identity", manifest_xml)),
        [ProblemKind::Reserved]
    );
}

#[test]
fn validation_enforces_add_replace_remove_semantics() {
    let fixture = Fixture::new();
    let shadow =
        "schema = 1\n[[add]]\npath = \"/system/etc/hosts\"\nsource = \"image/files/hosts\"\n";
    assert_eq!(fixture.problems(shadow), [ProblemKind::Shadows]);
    let shadow_link =
        "schema = 1\n[[add]]\npath = \"/system/vendor\"\nsource = \"image/files/hosts\"\n";
    assert_eq!(fixture.problems(shadow_link), [ProblemKind::Shadows]);

    let replace_missing = "schema = 1\n[[replace]]\npath = \"/system/etc/nope\"\nsource = \"image/files/hosts\"\nreason = \"r\"\n";
    assert_eq!(
        fixture.problems(replace_missing),
        [ProblemKind::MissingInOriginal]
    );
    let replace_dir = "schema = 1\n[[replace]]\npath = \"/system/etc\"\nsource = \"image/files/hosts\"\nreason = \"r\"\n";
    assert_eq!(
        fixture.problems(replace_dir),
        [ProblemKind::ReplacesDirectory]
    );
    let remove_missing = "schema = 1\n[[remove]]\npath = \"/system/app/Nope\"\nreason = \"r\"\n";
    assert_eq!(
        fixture.problems(remove_missing),
        [ProblemKind::MissingInOriginal]
    );
}

#[test]
fn validation_rejects_duplicates_and_overlaps() {
    let fixture = Fixture::new();
    let duplicate = r#"schema = 1
[[remove]]
path = "/system/etc/hosts"
reason = "a"
[[replace]]
path = "/system/etc/hosts"
source = "image/files/hosts"
reason = "b"
"#;
    assert_eq!(fixture.problems(duplicate), [ProblemKind::Duplicate]);
    let overlap = r#"schema = 1
[[add]]
path = "/system/app/Stock/extra.txt"
source = "image/files/hosts"
[[remove]]
path = "/system/app/Stock"
reason = "gone"
"#;
    assert_eq!(fixture.problems(overlap), [ProblemKind::Overlap]);
    // A shared prefix that is not an ancestor is not an overlap.
    let siblings = r#"schema = 1
[[add]]
path = "/vendor/etc/a"
source = "image/files/hosts"
[[add]]
path = "/vendor/etc/a-b"
source = "image/files/hosts"
"#;
    assert_eq!(fixture.problems(siblings), []);
}

#[test]
fn identity_is_stable_and_detects_changes() {
    let fixture = Fixture::new();
    let base = compute(&fixture, OVERLAY);
    assert_eq!(base, compute(&fixture, OVERLAY));

    // Order, comments and formatting do not matter.
    let reordered = r#"
# reordered
schema = 1
[[remove]]
reason = "no such hardware"
path = '/system/app/Stock'
[[add]]
path = "/vendor/etc/vintf/manifest.xml"
source = "_build/vendor/manifest.xml"
[[replace]]
path = "/system/etc/hosts"
source = "image/files/hosts"
reason = "IPv6 loopback"
"#;
    assert_eq!(base.hex, compute(&fixture, reordered).hex);

    // The receipt is the preimage.
    let (receipt_hash, _) = {
        let path = fixture.temp.0.join("receipt");
        fs::write(&path, &base.receipt).unwrap();
        darwin_android_image::plan::hash_file(&path).unwrap()
    };
    assert_eq!(receipt_hash, base.hex);

    let other_original = identity::compute(&"2".repeat(64), &fixture.plan(OVERLAY));
    assert_ne!(base.hex, other_original.hex, "original identity");
    assert_ne!(
        base.hex,
        compute(&fixture, &OVERLAY.replace("IPv6 loopback", "IPv6")).hex,
        "reason"
    );
    assert_ne!(base.hex, compute(&fixture, "schema = 1\n").hex, "entries");

    fs::write(
        fixture.sources.join("_build/vendor/manifest.xml"),
        "<manifest v=\"2\"/>\n",
    )
    .unwrap();
    assert_ne!(base.hex, compute(&fixture, OVERLAY).hex, "source content");
}

#[test]
fn original_identity_comes_from_file_or_flag() {
    let fixture = Fixture::new();
    let found = identity::original_identity(&fixture.original, None).unwrap();
    assert_eq!(found.as_deref(), Some(ORIGINAL_ID));
    let upper = ORIGINAL_ID.to_ascii_uppercase();
    assert_eq!(
        identity::original_identity(&fixture.original, Some(&upper))
            .unwrap()
            .as_deref(),
        Some(ORIGINAL_ID)
    );
    assert!(identity::original_identity(&fixture.original, Some(&"2".repeat(64))).is_err());
    assert!(identity::original_identity(&fixture.original, Some("xyz")).is_err());
    let bare = fixture.temp.0.join("bare");
    fs::create_dir(&bare).unwrap();
    assert_eq!(identity::original_identity(&bare, None).unwrap(), None);
}

#[test]
fn assemble_applies_the_overlay_read_only() {
    let fixture = Fixture::new();
    let plan = fixture.plan(OVERLAY);
    let identity = identity::compute(ORIGINAL_ID, &plan);
    let out = fixture.temp.0.join("images/derived");

    let outcome = assemble(&plan, &fixture.original, &identity, &out).unwrap();
    assert_eq!(outcome, Outcome::Built);

    assert_eq!(
        fs::read_to_string(out.join("system/etc/hosts")).unwrap(),
        "127.0.0.1 localhost\n::1 localhost\n"
    );
    assert_eq!(
        fs::read_to_string(out.join("vendor/etc/vintf/manifest.xml")).unwrap(),
        "<manifest/>\n"
    );
    assert!(!out.join("system/app/Stock").exists());
    assert!(out.join("system/app").is_dir());
    assert_eq!(
        fs::read_to_string(out.join("system/bin/toybox")).unwrap(),
        "#!elf\n"
    );
    assert_eq!(
        fs::read_link(out.join("system/vendor")).unwrap(),
        Path::new("/system/vendor-real")
    );

    // Identity receipt.
    assert_eq!(
        fs::read_to_string(out.join(".identity")).unwrap(),
        format!("{}\n", identity.hex)
    );
    assert_eq!(
        fs::read_to_string(out.join(".overlay-receipt")).unwrap(),
        identity.receipt
    );

    // Read-only; execute bits survive.
    for directory in ["", "system", "system/app", "vendor/etc/vintf"] {
        assert_eq!(mode(&out.join(directory)), 0o555, "{directory}");
    }
    for file in [
        "system/etc/hosts",
        "vendor/etc/vintf/manifest.xml",
        ".identity",
    ] {
        assert_eq!(mode(&out.join(file)), 0o444, "{file}");
    }
    assert_eq!(mode(&out.join("system/bin/toybox")), 0o555);

    // The original is untouched.
    assert_eq!(
        fs::read_to_string(fixture.original.join("system/etc/hosts")).unwrap(),
        "127.0.0.1 localhost\n"
    );
    assert!(
        fixture
            .original
            .join("system/app/Stock/Stock.apk")
            .is_file()
    );
    assert!(!fixture.original.join("vendor/etc/vintf").exists());
    assert_eq!(
        fs::read_to_string(fixture.original.join(".identity")).unwrap(),
        format!("{ORIGINAL_ID}\n")
    );
    assert_eq!(mode(&fixture.original.join("system/bin/toybox")), 0o755);
    assert_ne!(
        fs::metadata(fixture.original.join("system/bin/toybox"))
            .unwrap()
            .ino(),
        fs::metadata(out.join("system/bin/toybox")).unwrap().ino(),
        "a clone, not a hard link"
    );

    // No staging directory left behind.
    let leftovers: Vec<_> = fs::read_dir(out.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["derived"]);
}

#[test]
fn assemble_reuses_the_same_identity_and_refuses_others() {
    let fixture = Fixture::new();
    let plan = fixture.plan(OVERLAY);
    let identity = identity::compute(ORIGINAL_ID, &plan);
    let out = fixture.temp.0.join("derived");
    assert_eq!(
        assemble(&plan, &fixture.original, &identity, &out).unwrap(),
        Outcome::Built
    );
    let before = fs::metadata(out.join("system/etc/hosts")).unwrap();

    assert_eq!(
        assemble(&plan, &fixture.original, &identity, &out).unwrap(),
        Outcome::Reused
    );
    let after = fs::metadata(out.join("system/etc/hosts")).unwrap();
    assert_eq!(
        (before.ino(), before.mtime_nsec()),
        (after.ino(), after.mtime_nsec())
    );

    // A different identity never overwrites an existing image.
    let empty = fixture.plan("schema = 1\n");
    let other = identity::compute(ORIGINAL_ID, &empty);
    let error = assemble(&empty, &fixture.original, &other, &out).unwrap_err();
    assert!(error.contains("never rebuilt in place"), "{error}");
    assert_eq!(
        fs::read_to_string(out.join(".identity")).unwrap(),
        format!("{}\n", identity.hex)
    );

    // Nor any directory that is not an image.
    let stranger = fixture.temp.0.join("stranger");
    fs::create_dir(&stranger).unwrap();
    assert!(assemble(&empty, &fixture.original, &other, &stranger).is_err());
}

#[test]
fn empty_manifest_derives_the_original_with_a_new_identity() {
    let fixture = Fixture::new();
    let plan = fixture.plan("schema = 1\n");
    let identity = identity::compute(ORIGINAL_ID, &plan);
    assert_ne!(identity.hex, ORIGINAL_ID);
    let out = fixture.temp.0.join("derived");
    assemble(&plan, &fixture.original, &identity, &out).unwrap();
    assert!(out.join("system/app/Stock/oat/arm64/Stock.odex").is_file());
    assert_eq!(
        fs::read_to_string(out.join(".identity")).unwrap(),
        format!("{}\n", identity.hex)
    );
}

#[test]
fn diff_lists_every_deviation_with_sizes_and_reasons() {
    let fixture = Fixture::new();
    let plan = fixture.plan(OVERLAY);
    let identity = identity::compute(ORIGINAL_ID, &plan);
    let text = diff::render(
        &Header {
            manifest: "image/overlay.toml",
            original: "ORIG",
            original_identity: Some(ORIGINAL_ID),
            derived_identity: Some(&identity.hex),
        },
        &plan,
    );
    let hosts_sha = &plan.steps[1].source.as_ref().unwrap().sha256;
    let manifest_sha = &plan.steps[2].source.as_ref().unwrap().sha256;
    let expected = format!(
        "\
original  ORIG
          identity {ORIGINAL_ID}
manifest  image/overlay.toml
derived   identity {id}

- remove   /system/app/Stock               directory, 2 files, 13 B
             reason no such hardware
~ replace  /system/etc/hosts               20 B -> 34 B (+14 B)
             from   image/files/hosts (sha256 {hosts_sha})
             reason IPv6 loopback
+ add      /vendor/etc/vintf/manifest.xml  12 B
             from   _build/vendor/manifest.xml (sha256 {manifest_sha})

1 added (+12 B), 1 replaced (+14 B), 1 removed (-13 B)
",
        id = identity.hex
    );
    assert_eq!(text, expected);

    let empty = diff::render(
        &Header {
            manifest: "m",
            original: "o",
            original_identity: None,
            derived_identity: None,
        },
        &fixture.plan("schema = 1\n"),
    );
    assert!(empty.contains("no deviations"), "{empty}");
    assert!(empty.contains("identity unknown"), "{empty}");
}

#[test]
fn human_sizes() {
    assert_eq!(diff::human(0), "0 B");
    assert_eq!(diff::human(1023), "1023 B");
    assert_eq!(diff::human(1536), "1.5 KiB");
    assert_eq!(diff::human(3 * 1024 * 1024 * 1024), "3.0 GiB");
}
