//! Original Java syntax behind the production parser attachment, without a guest boot.
//! Requires the pinned host JDK and network access to hash-pinned AOSP sources.
use super::*;
use aim_service_aidl::dev_aim_server_ipackageintentparser as parser;
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};

pub(super) struct JavaParser(PathBuf);

fn run(command: &mut Command) -> Vec<u8> {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{command:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty(), "{command:?}: {:?}", output.stderr);
    output.stdout
}

fn original(dir: &Path, project: &str, path: &str, hash: &str) -> String {
    let encoded = dir.join("source.base64");
    fs::write(
        &encoded,
        run(Command::new("curl").args([
            "-fsSL",
            "--max-time",
            "30",
            &format!("https://android.googlesource.com/platform/{project}/+/android-16.0.0_r1/{path}?format=TEXT"),
        ])),
    )
    .unwrap();
    let bytes = run(Command::new("base64").arg("-d").arg("-i").arg(encoded));
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), hash);
    String::from_utf8(bytes).unwrap()
}

fn method<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
    let source = &source[source.find(start).unwrap()..];
    &source[..source.find(end).unwrap()]
}

impl JavaParser {
    pub(super) fn new(root: &Path) -> Self {
        let dir = root.join("intent-parser");
        fs::create_dir(&dir).unwrap();
        let owner = Self(dir);
        let dir = &owner.0;
        let source = original(
            dir,
            "frameworks/base",
            "core/java/android/content/Intent.java",
            "4dc0215006dff72017b05a18451e4090385271bd511d0c399f0a15782e3d79d1",
        );
        let parse = method(
            &source,
            "    public static Intent parseCommandArgs(",
            "\n    /** @hide */",
        );
        let decode = method(
            &source,
            "    private static Integer decodeInteger(",
            "\n    /** @hide */",
        );
        // Include the original constant expressions, including aliases used by flags.
        let constants = source
            .lines()
            .filter(|line| {
                line.trim_start().starts_with("public static final int ")
                    && ["FLAG_", "URI_", "FILL_IN_"]
                        .iter()
                        .any(|prefix| line.contains(prefix))
            })
            .map(|line| {
                let declaration = &source[source.find(line).unwrap()..];
                &declaration[..=declaration.find(';').unwrap()]
            })
            .collect::<Vec<_>>()
            .join("\n");
        let fixture = aim_paths::root().join("crates/aim-services/tests/fixtures/package_intent");
        let intent = fs::read_to_string(fixture.join("android/content/Intent.java")).unwrap()
            .replace("// ORIGINAL_CONSTANTS", &format!("{constants}\n public static final String ACTION_MAIN=\"android.intent.action.MAIN\", CATEGORY_LAUNCHER=\"android.intent.category.LAUNCHER\";"))
            .replace("// ORIGINAL_METHODS", &format!("{decode}\n{parse}"));
        fs::write(dir.join("Intent.java"), intent).unwrap();
        let component = original(
            dir,
            "frameworks/base",
            "core/java/android/content/ComponentName.java",
            "8077358457df668ce7d2f92c058ceaab6562e13d9026ccf85f98ca85d62e1ad4",
        );
        let unflatten = method(
            &component,
            "    public static @Nullable ComponentName unflattenFromString(",
            "\n    /**",
        );
        let component = fs::read_to_string(fixture.join("android/content/ComponentName.java"))
            .unwrap()
            .replace("// ORIGINAL_METHODS", unflatten);
        fs::write(dir.join("ComponentName.java"), component).unwrap();
        let basic = original(
            dir,
            "frameworks/libs/modules-utils",
            "java/com/android/modules/utils/BasicShellCommandHandler.java",
            "f56b4ac8f89b0361e31d63790357fc108e6d3d65b4b0338c67ca6c62e8f48bf9",
        );
        fs::write(dir.join("BasicShellCommandHandler.java"), basic).unwrap();
        let production = aim_paths::root().join("java/device-services");
        let mut compile = Command::new(Self::jdk().join("javac"));
        compile
            .args(["--release", "17", "-d"])
            .arg(dir)
            .arg("-sourcepath")
            .arg(production.join("stubs"))
            .arg(dir.join("Intent.java"))
            .arg(dir.join("ComponentName.java"))
            .arg(dir.join("BasicShellCommandHandler.java"))
            .arg(production.join("src/dev/aim/server/PackageIntentParser.java"));
        for path in [
            "android/os/Parcel.java",
            "android/os/ShellCommand.java",
            "android/os/Bundle.java",
            "android/util/Log.java",
            "dev/aim/server/Bridge.java",
            "dev/aim/server/IPackageIntentParser.java",
            "dev/aim/server/PackageIntentParserTest.java",
        ] {
            compile.arg(fixture.join(path));
        }
        compile
            .arg(aim_paths::root().join(
                "crates/aim-services/tests/fixtures/domain_shell/android/os/UserHandle.java",
            ));
        run(&mut compile);
        owner
    }
    fn jdk() -> PathBuf {
        aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin")
    }
}
impl Service for JavaParser {
    fn descriptor(&self) -> &str {
        parser::DESCRIPTOR
    }
    fn transact(&self, call: &mut Call<'_>) -> Reply {
        assert_eq!(call.code, parser::PARSE_INTENT);
        assert_eq!(call.sender_euid, 1000);
        let args = parser::ParseIntent::read(&mut call.data)?
            .arguments
            .unwrap();
        assert_eq!(call.data.remaining(), 0);
        let bytes = run(Command::new(Self::jdk().join("java"))
            .arg("-cp")
            .arg(&self.0)
            .arg("dev.aim.server.PackageIntentParserTest")
            .args(args.into_iter().map(Option::unwrap)));
        let mut p = Parcel::new();
        parser::write_parse_intent_reply(&mut p, &Some(bytes));
        Ok(p)
    }
}
