//! The Java toolchain of `upstream/java-toolchain.lock` (docs/build.md,
//! "Java"): a pinned JDK, the SDK build tools' d8, aapt2, apksigner and
//! aidl, and AOSP's public test key. Each archive is downloaded into
//! `_build/downloads` once, checked against its pin and unpacked into
//! `_build/java`; nothing of it goes into the image.
//!
//! Code for system_server is compiled against stubs of the image's
//! internal classes, then checked against the image's own jars
//! (`aim_android_image::linkage`): a changed internal API fails the build.

use super::repo;
use crate::fetch::{self, Source};
use crate::hash;
use crate::lockfile::Lock;
use crate::log::Log;
use aim_android_image::linkage::ClassPath;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub const LOCK: &str = "upstream/java-toolchain.lock";

/// The Android API level of the image, what the code is dexed for.
const MIN_API: &str = "36";

pub struct Toolchain {
    /// The JDK's home.
    jdk: PathBuf,
    build_tools: PathBuf,
    key: PathBuf,
    cert: PathBuf,
}

/// Downloads `url` once and checks its sha256.
fn pinned(url: &str, name: &str, sha256: &str, log: &mut Log) -> Result<PathBuf, String> {
    let file = aim_paths::downloads().join(name);
    if !file.exists() {
        fetch::download(url, &file, log, |_, _| true)?;
    }
    let got = hash::sha256_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    if got != sha256 {
        return Err(format!(
            "{}: sha256 {got}, not the pin {sha256} of {LOCK}",
            file.display()
        ));
    }
    Ok(file)
}

/// Unpacks `archive` into `_build/java/<name>` once (`.fetched` marks a
/// complete one).
fn unpacked(archive: &Path, name: &str, log: &mut Log) -> Result<PathBuf, String> {
    let dest = aim_paths::fetched().join("java").join(name);
    if dest.join(".fetched").exists() {
        return Ok(dest);
    }
    let work = dest.with_extension("partial");
    let _ = fs::remove_dir_all(&work);
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    if archive.extension().is_some_and(|e| e == "zip") {
        log.run(
            Command::new("unzip")
                .arg("-q")
                .arg(archive)
                .arg("-d")
                .arg(&work),
        )?;
    } else {
        log.run(
            Command::new("tar")
                .arg("-xzf")
                .arg(archive)
                .arg("-C")
                .arg(&work),
        )?;
    }
    fs::write(work.join(".fetched"), "").map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(&dest);
    fs::rename(&work, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

/// The pinned JDK, also used by ART and xsdc without the SDK build tools.
pub fn jdk(log: &mut Log) -> Result<PathBuf, String> {
    // Independent nodes can fetch the same archive and extraction concurrently.
    static FETCH: Mutex<()> = Mutex::new(());
    let _fetch = FETCH.lock().unwrap();
    let lock = Lock::read(&repo(LOCK))?;
    let version = lock.get("JDK_VERSION")?;
    let archive = pinned(
        lock.get("JDK_URL")?,
        &format!("temurin-jdk-{version}.tar.gz"),
        lock.get("JDK_SHA256")?,
        log,
    )?;
    Ok(unpacked(&archive, &format!("temurin-{version}"), log)?
        .join(format!("jdk-{version}/Contents/Home")))
}

impl Toolchain {
    /// The pinned toolchain, fetched if needed.
    pub fn fetch(log: &mut Log) -> Result<Self, String> {
        let lock = Lock::read(&repo(LOCK))?;
        let jdk = jdk(log)?;
        let version = lock.get("BUILD_TOOLS_VERSION")?;
        let tools = pinned(
            lock.get("BUILD_TOOLS_URL")?,
            &format!("build-tools-{version}-macosx.zip"),
            lock.get("BUILD_TOOLS_SHA256")?,
            log,
        )?;
        let build_tools = unpacked(&tools, &format!("build-tools-{version}"), log)?
            .join(lock.get("BUILD_TOOLS_DIR")?);
        let tag = lock.get("AOSP_TAG")?;
        let [key, cert] = lock.array("TEST_KEY") else {
            return Err(format!(
                "{LOCK}: TEST_KEY names the key and its certificate"
            ));
        };
        Ok(Self {
            jdk,
            build_tools,
            key: Source::parse(key, tag)?.file(log)?,
            cert: Source::parse(cert, tag)?.file(log)?,
        })
    }

    pub(super) fn java(&self) -> Command {
        Command::new(self.jdk.join("bin/java"))
    }

    /// Compiles `sources` (every `.java` under them) against `classpath`
    /// into `out`, for the Java 17 language and library; with `lint`,
    /// every warning is an error.
    pub fn javac(
        &self,
        log: &mut Log,
        sources: &[PathBuf],
        classpath: &[PathBuf],
        out: &Path,
        lint: bool,
    ) -> Result<(), String> {
        let mut files = Vec::new();
        for dir in sources {
            files.extend(
                hash::files_under(dir)
                    .into_iter()
                    .filter(|f| f.extension().is_some_and(|e| e == "java")),
            );
        }
        let mut cmd = Command::new(self.jdk.join("bin/javac"));
        cmd.args(["--release", "17", "-encoding", "UTF-8", "-Xmaxerrs", "1000"])
            .arg("-d")
            .arg(out);
        if lint {
            cmd.args(["-Xlint:all", "-Werror"]);
        }
        if !classpath.is_empty() {
            cmd.arg("-classpath").arg(join(classpath));
        }
        log.run(cmd.args(files))
    }

    /// Generates the Java of the AIDL files under `dir` into `out`.
    pub fn aidl(&self, log: &mut Log, dir: &Path, out: &Path) -> Result<(), String> {
        let files: Vec<PathBuf> = hash::files_under(dir)
            .into_iter()
            .filter(|f| f.extension().is_some_and(|e| e == "aidl"))
            .collect();
        log.run(
            Command::new(self.build_tools.join("aidl"))
                .args(["--lang=java", "--min_sdk_version", MIN_API])
                .arg(format!("-I{}", dir.display()))
                .arg("-o")
                .arg(out)
                .args(files),
        )
    }

    /// Dexes the class files under `classes` (compiled against
    /// `classpath`) into `out/classes.dex`.
    pub fn d8(
        &self,
        log: &mut Log,
        classes: &Path,
        classpath: Option<&Path>,
        out: &Path,
    ) -> Result<PathBuf, String> {
        let files: Vec<PathBuf> = hash::files_under(classes)
            .into_iter()
            .filter(|f| f.extension().is_some_and(|e| e == "class"))
            .collect();
        fs::create_dir_all(out).map_err(|e| e.to_string())?;
        let mut cmd = self.java();
        cmd.arg("-cp")
            .arg(self.build_tools.join("lib/d8.jar"))
            .arg("com.android.tools.r8.D8")
            .args(["--release", "--min-api", MIN_API])
            .arg("--lib")
            .arg(&self.jdk);
        if let Some(classpath) = classpath {
            cmd.arg("--classpath").arg(classpath);
        }
        log.run(cmd.arg("--output").arg(out).args(files))?;
        Ok(out.join("classes.dex"))
    }

    /// A jar of `dex` as the platform's are: stored and aligned, so ART
    /// maps it in place.
    pub fn jar(&self, log: &mut Log, dex: &Path, out: &Path) -> Result<(), String> {
        let unaligned = out.with_extension("unaligned");
        log.run(
            Command::new(self.jdk.join("bin/jar"))
                .arg("--create")
                .arg("--no-compress")
                .arg("--no-manifest")
                .arg("--file")
                .arg(&unaligned)
                .arg("-C")
                .arg(dex.parent().unwrap())
                .arg(dex.file_name().unwrap()),
        )?;
        log.run(
            Command::new(self.build_tools.join("zipalign"))
                .args(["-f", "4"])
                .arg(&unaligned)
                .arg(out),
        )?;
        fs::remove_file(&unaligned).map_err(|e| e.to_string())
    }

    /// Checks the dex files of `jar` as ART opens them: the build tools'
    /// dexdump runs ART's `DexFileVerifier` on each (`-c`: and dumps
    /// nothing).
    pub fn verify_dex(&self, log: &mut Log, jar: &Path) -> Result<(), String> {
        log.run(
            Command::new(self.build_tools.join("dexdump"))
                .arg("-c")
                .arg(jar),
        )
    }

    /// Builds and signs the APK of `manifest`, the resources under `res`
    /// and the code of `dex` (a `classes.dex`), linked against the image's
    /// `framework-res.apk`, into `out`.
    pub fn apk(
        &self,
        log: &mut Log,
        manifest: &Path,
        res: Option<&Path>,
        dex: Option<&Path>,
        framework: &Path,
        out: &Path,
    ) -> Result<(), String> {
        let work = out.with_extension("work");
        let _ = fs::remove_dir_all(&work);
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let aapt2 = self.build_tools.join("aapt2");
        let mut link = Command::new(&aapt2);
        link.arg("link")
            .arg("--manifest")
            .arg(manifest)
            .arg("-I")
            .arg(framework)
            .args([
                "--min-sdk-version",
                MIN_API,
                "--target-sdk-version",
                MIN_API,
            ]);
        if let Some(res) = res {
            let compiled = work.join("res.zip");
            log.run(
                Command::new(&aapt2)
                    .arg("compile")
                    .arg("--dir")
                    .arg(res)
                    .arg("-o")
                    .arg(&compiled),
            )?;
            link.arg(compiled);
        }
        let unsigned = work.join("unsigned.apk");
        log.run(link.arg("-o").arg(&unsigned))?;
        if let Some(dex) = dex {
            log.run(
                Command::new(self.jdk.join("bin/jar"))
                    .arg("--update")
                    .arg("--no-compress")
                    .arg("--no-manifest")
                    .arg("--file")
                    .arg(&unsigned)
                    .arg("-C")
                    .arg(dex.parent().unwrap())
                    .arg(dex.file_name().unwrap()),
            )?;
        }
        let aligned = work.join("aligned.apk");
        log.run(
            Command::new(self.build_tools.join("zipalign"))
                .args(["-f", "4"])
                .arg(&unsigned)
                .arg(&aligned),
        )?;
        log.run(
            self.java()
                .arg("-jar")
                .arg(self.build_tools.join("lib/apksigner.jar"))
                .arg("sign")
                .args(["--v4-signing-enabled", "false"])
                .arg("--key")
                .arg(&self.key)
                .arg("--cert")
                .arg(&self.cert)
                .arg("--out")
                .arg(out)
                .arg(&aligned),
        )?;
        fs::remove_dir_all(&work).map_err(|e| e.to_string())
    }
}

/// Fails unless the stubs in `stubs`, if given, declare what the image's
/// jars on `jars` (guest paths) declare, and everything `dex` refers to
/// outside itself is in those jars.
pub fn check_linkage(
    image: &Path,
    jars: &[String],
    stubs: Option<&Path>,
    dex: &Path,
) -> Result<(), String> {
    check_linkage_on(ClassPath::read(image, jars)?, stubs, dex)
}

pub fn check_linkage_on(
    classpath: ClassPath,
    stubs: Option<&Path>,
    dex: &Path,
) -> Result<(), String> {
    let read = |p: &Path| fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    let mut problems = match stubs {
        Some(stubs) => classpath.stub_mismatches(&read(stubs)?)?,
        None => Vec::new(),
    };
    let code = read(dex)?;
    problems.extend(classpath.unresolved(&code)?);
    for (adapter, parent, methods) in [
        (
            "Ldev/aim/server/NativeSyncAccounts;",
            "Lcom/android/server/content/SyncAccounts;",
            &[
                ("validateAccounts", "(I)V"),
                (
                    "getAccounts",
                    "(ILjava/lang/String;)[Landroid/accounts/Account;",
                ),
                (
                    "getRunningAccountsForSystem",
                    "()[Landroid/accounts/AccountAndUser;",
                ),
                (
                    "getAllAccountsForSystemProcess",
                    "()[Landroid/accounts/AccountAndUser;",
                ),
            ][..],
        ),
        (
            "Ldev/aim/server/ActivityStats;",
            "Landroid/app/ActivityManagerInternal;",
            &[
                ("isUidActive", "(I)Z"),
                ("getUidProcessState", "(I)I"),
                ("isAppStartModeDisabled", "(ILjava/lang/String;)Z"),
                ("getRestrictionLevel", "(I)I"),
            ][..],
        ),
        (
            "Ldev/aim/server/PackageLocalOwners$Packages;",
            "Landroid/content/pm/PackageManagerInternal;",
            &[
                ("filterAppAccess", "(Ljava/lang/String;II)Z"),
                ("filterAppAccess", "(Ljava/lang/String;IIZ)Z"),
                (
                    "filterAppAccess",
                    "(Lcom/android/server/pm/pkg/AndroidPackage;II)Z",
                ),
                ("wasPackageEverLaunched", "(Ljava/lang/String;I)Z"),
                ("isPackageStopped", "(Ljava/lang/String;I)Z"),
            ][..],
        ),
        (
            "Lcom/android/server/pm/permission/AppHostDefaultPermissionGrants$Local;",
            "Lcom/android/server/pm/permission/LegacyPermissionManagerInternal;",
            &[(
                "setSyncAdapterPackagesProvider",
                "(Lcom/android/server/pm/permission/LegacyPermissionManagerInternal$SyncAdapterPackagesProvider;)V",
            )][..],
        ),
    ] {
        if aim_android_image::dex::Dex::parse(&code)?
            .class(adapter)
            .is_some()
        {
            problems.extend(classpath.local_contract(&code, adapter, parent, methods)?);
        }
    }
    if !problems.is_empty() {
        return Err(format!(
            "{} does not link against the image:\n  {}",
            dex.display(),
            problems.join("\n  ")
        ));
    }
    Ok(())
}

fn join(paths: &[PathBuf]) -> String {
    let paths: Vec<String> = paths.iter().map(|p| p.display().to_string()).collect();
    paths.join(":")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_contract_rejects_inherited_abstract_methods_and_wrong_overloads() {
        let work = aim_paths::scratch_dir("local-contract");
        let source = work.join("source");
        fs::create_dir_all(&source).unwrap();
        let mut log = Log::create(work.join("test.log"), false).unwrap();
        let tools = Toolchain::fetch(&mut log).unwrap();
        // Compile against the incomplete API, then link against its real shape.
        fs::write(
            source.join("Base.java"),
            "public abstract class Base { public Base() {} }",
        )
        .unwrap();
        fs::write(
            source.join("Missing.java"),
            "public class Missing extends Base {}",
        )
        .unwrap();
        fs::write(
            source.join("Good.java"),
            "public class Good extends Base { public int query(int uid) { return uid; } }",
        )
        .unwrap();
        fs::write(
            source.join("Inherited.java"),
            "public class Inherited extends Good {}",
        )
        .unwrap();
        fs::write(
            source.join("Wrong.java"),
            "public class Wrong extends Base { public int query(String uid) { return 0; } }",
        )
        .unwrap();
        let classes = work.join("classes");
        tools
            .javac(&mut log, &[source.clone()], &[], &classes, false)
            .unwrap();
        fs::remove_file(classes.join("Base.class")).unwrap();
        let adapters = tools
            .d8(&mut log, &classes, None, &work.join("adapters"))
            .unwrap();
        let real = work.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::write(
            real.join("Base.java"),
            "public abstract class Base { public abstract int query(int uid); }",
        )
        .unwrap();
        let api = work.join("api");
        // Only the real base is compiled; incomplete adapters would fail javac here.
        tools.javac(&mut log, &[real], &[], &api, false).unwrap();
        let api_dex = tools
            .d8(&mut log, &api, None, &work.join("api-dex"))
            .unwrap();
        tools
            .jar(&mut log, &api_dex, &work.join("api.jar"))
            .unwrap();
        let path = ClassPath::read(&work, &["api.jar".into()]).unwrap();
        let dex = fs::read(adapters).unwrap();
        let methods = &[("query", "(I)I")];
        for adapter in ["LGood;", "LInherited;"] {
            assert!(
                path.local_contract(&dex, adapter, "LBase;", methods)
                    .unwrap()
                    .is_empty()
            );
        }
        for adapter in ["LMissing;", "LWrong;", "LBase;"] {
            assert!(
                path.local_contract(&dex, adapter, "LBase;", methods)
                    .unwrap()[0]
                    .contains("no concrete local method")
            );
        }
        assert!(
            path.local_contract(&dex, "LGood;", "LBase;", &[("absent", "()V")])
                .unwrap()
                .iter()
                .any(|s| s.contains("no local API"))
        );
    }
}
