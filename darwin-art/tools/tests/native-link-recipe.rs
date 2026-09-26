use super::*;
use std::os::unix::ffi::OsStringExt;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        // Parallel tests can read the same clock value (#21).
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "darwin-art-native-link-recipe-test-{}-{}-{serial}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn identity(dir: &Path, stem: &str) -> LinkIdentity {
    LinkIdentity {
        output: dir.join(format!("{stem} output.dylib")),
        map: dir.join(format!("{stem} map.txt")),
        install_name: OsString::from(format!("@rpath/lib{stem}.dylib")),
    }
}

fn recipe_command(dir: &Path, product: &LinkIdentity) -> (Command, PathBuf, PathBuf) {
    let object = dir.join("input with space.o");
    let archive = dir.join("force loaded.a");
    let export_list = dir.join("exports list.txt");
    fs::write(&object, b"object").unwrap();
    fs::write(&archive, b"archive").unwrap();
    fs::write(&export_list, b"_entry\n").unwrap();
    let non_utf8_argument = OsString::from_vec(vec![b'-', b'D', b'n', 0x80]);

    let mut command = Command::new("clang++");
    command
        .arg("-dynamiclib")
        .arg(format!("-Wl,-map,{}", product.map.display()))
        .arg(format!(
            "-Wl,-install_name,{}",
            product.install_name.to_string_lossy()
        ))
        .args(["-Xlinker", "-exported_symbols_list", "-Xlinker"])
        .arg(&export_list)
        .arg(format!("-Wl,-force_load,{}", archive.display()))
        .args(["-Xlinker", "-force_load", "-Xlinker"])
        .arg(&archive)
        .arg(&object)
        .arg("-Wl,-rpath,@loader_path")
        .arg(OsString::new())
        .arg(non_utf8_argument)
        .arg("-o")
        .arg(&product.output);
    (command, object, export_list)
}

#[test]
fn round_trip_preserves_order_empty_and_non_utf8_fields() {
    let temp = TempDir::new();
    let product = identity(temp.path(), "product");
    let (command, _object, export_list) = recipe_command(temp.path(), &product);
    let recipe = temp.path().join("recipe with spaces.bin");
    write_successful_recipe(&recipe, &command).unwrap();

    let test = identity(temp.path(), "test");
    let test_object = temp.path().join("test.o");
    fs::write(&test_object, b"test").unwrap();
    let transformed = transform_recipe(
        &recipe,
        &product,
        &test,
        &test_object,
        OsStr::new("_darwin_art_test_entry"),
    )
    .unwrap();
    let args = transformed
        .get_args()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let original = command
        .get_args()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    assert_eq!(args.len(), original.len() + 2);
    for (index, before) in original.iter().enumerate() {
        let expected = if before.as_bytes().starts_with(b"-Wl,-map,") {
            OsString::from(format!("-Wl,-map,{}", test.map.display()))
        } else if before.as_bytes().starts_with(b"-Wl,-install_name,") {
            OsString::from(format!(
                "-Wl,-install_name,{}",
                test.install_name.to_string_lossy()
            ))
        } else if before == product.output.as_os_str() {
            test.output.clone().into_os_string()
        } else {
            before.clone()
        };
        assert_eq!(
            args[index], expected,
            "argument order/root changed at {index}"
        );
    }
    assert!(args.iter().any(|arg| arg == &test.output.as_os_str()));
    assert!(args.iter().any(|arg| arg == &test_object.as_os_str()));
    assert!(args
        .iter()
        .any(|arg| { arg.as_bytes() == b"-Wl,-exported_symbol,_darwin_art_test_entry" }));
    assert!(args.iter().any(|arg| arg == &export_list.as_os_str()));
    assert!(args
        .windows(2)
        .any(|pair| { pair[0].is_empty() && pair[1].as_bytes().starts_with(b"-D") }));
    assert!(args
        .iter()
        .any(|arg| arg.as_bytes() == [b'-', b'D', b'n', 0x80]));
}

#[test]
fn rejects_truncation_duplicate_outputs_and_unsupported_spellings() {
    let temp = TempDir::new();
    let product = identity(temp.path(), "product");
    let mut command = Command::new("clang++");
    command.args([
        "-dynamiclib",
        "-Wl,-map,/tmp/a",
        "-Wl,-map,/tmp/b",
        "-Wl,-install_name,@rpath/product.dylib",
    ]);
    assert!(validate_command(&command).is_err());

    for spelling in [
        "-o/tmp/product",
        "-Wl,-map=/tmp/map",
        "-Wl,-install_name=/tmp/name",
    ] {
        let mut invalid_command = Command::new("clang++");
        invalid_command.args(["-dynamiclib", spelling]);
        assert!(validate_command(&invalid_command).is_err(), "{spelling}");
    }
    let mut xlinker = Command::new("clang++");
    xlinker.args(["-dynamiclib", "-Xlinker", "-o", "-Xlinker", "/tmp/out"]);
    assert!(validate_command(&xlinker).is_err());

    let recipe = temp.path().join("truncated");
    fs::write(&recipe, [MAGIC, b"\0", VERSION, b"\0", b"clang++"].concat()).unwrap();
    assert!(read_recipe(&recipe).is_err());
    fs::create_dir(temp.path().join("nested")).unwrap();
    assert!(paths_alias(
        &product.output,
        &temp.path().join("nested/../product output.dylib")
    ));
}

#[test]
fn rejects_explicit_environment_and_missing_input_artifacts() {
    let mut command = Command::new("clang++");
    command.env("DARWIN_ART_TEST", "1");
    assert!(validate_command(&command).is_err());
    let mut command = Command::new("clang++");
    command.args([
        "-dynamiclib",
        "-Wl,-map,/tmp/map",
        "-Wl,-install_name,@rpath/product.dylib",
        "-Wl,-force_load,/tmp/missing.a",
    ]);
    assert!(validate_command(&command).is_err());
}

#[test]
fn rejects_output_aliases_including_cross_product_archive_and_symlink() {
    let temp = TempDir::new();
    let product = identity(temp.path(), "product");
    let (command, object, exports) = recipe_command(temp.path(), &product);
    let recipe = temp.path().join("recipe");
    write_successful_recipe(&recipe, &command).unwrap();
    let test_object = temp.path().join("test.o");
    fs::write(&test_object, b"test").unwrap();
    for alias in [
        &product.output,
        &product.map,
        &object,
        &exports,
        &temp.path().join("force loaded.a"),
        &test_object,
    ] {
        let mut test = identity(temp.path(), "test");
        test.output = alias.clone();
        assert!(
            transform_recipe(
                &recipe,
                &product,
                &test,
                &test_object,
                OsStr::new("_fixture")
            )
            .is_err(),
            "alias {}",
            alias.display()
        );
    }
    fs::write(&product.output, b"product").unwrap();
    let symlink = temp.path().join("alias.dylib");
    std::os::unix::fs::symlink(&product.output, &symlink).unwrap();
    let mut test = identity(temp.path(), "test");
    test.output = symlink;
    assert!(transform_recipe(
        &recipe,
        &product,
        &test,
        &test_object,
        OsStr::new("_fixture")
    )
    .is_err());
    let mut test = identity(temp.path(), "test");
    test.map = product.output.clone();
    assert!(transform_recipe(
        &recipe,
        &product,
        &test,
        &test_object,
        OsStr::new("_fixture")
    )
    .is_err());
}

#[test]
fn rejects_response_compiler_identity_and_header_corruption() {
    let temp = TempDir::new();
    let product = identity(temp.path(), "product");
    let (mut command, _, _) = recipe_command(temp.path(), &product);
    command.arg("@hidden-response");
    assert!(validate_command(&command).is_err());
    let mut impostor = Command::new("/tmp/clang++");
    impostor.args(command.get_args());
    assert!(validate_command(&impostor).is_err());
    let recipe = temp.path().join("recipe");
    fs::write(&recipe, b"WRONG\0").unwrap();
    assert!(read_recipe(&recipe).is_err());
    fs::write(&recipe, [MAGIC, b"\0", b"2\0clang++\0"].concat()).unwrap();
    assert!(read_recipe(&recipe).is_err());
}
