use super::*;

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        writable_directories(&self.0);
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn prepared_identity_comes_from_archive_and_survives_read_only_reuse() {
    let fixture = Fixture(std::env::temp_dir().join(format!("dar-image-id-{}-{}",
        std::process::id(), std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos())));
    fs::create_dir(&fixture.0).unwrap();
    let source = fixture.0.join("source");
    for name in REQUIRED {
        let path = source.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, name.as_bytes()).unwrap();
    }
    let archive = fixture.0.join("image.tar");
    assert!(
        Command::new("/usr/bin/tar")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .args(["apex", "system", "linkerconfig"])
            .status()
            .unwrap()
            .success()
    );
    let expected: [u8; 32] = Sha256::digest(fs::read(&archive).unwrap()).into();
    let store = fixture.0.join("store");
    let first = prepare_with_identity(&archive, &store).unwrap();
    assert_eq!(first.content_id, expected);
    let inode = fs::metadata(&first.root).unwrap().ino();
    let second = prepare_with_identity(&archive, &store).unwrap();
    assert_eq!(second.content_id, expected);
    assert_eq!(second.root, first.root);
    assert_eq!(fs::metadata(&second.root).unwrap().ino(), inode);
    assert_eq!(fs::metadata(&second.root).unwrap().mode() & 0o222, 0);
    assert_eq!(prepare(&archive, &store).unwrap(), second.root);
}
