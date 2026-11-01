//! #1460: typed, per-instance account access in the pinned services DEX.
//! Static redirects cannot change getSingleton's concrete return type or add
//! constructor state. Reassemble the original DEX with a hash-checked patch;
//! do not rebuild Android sources or shadow any original class in a vendor jar.

use super::java::{self, Toolchain};
use super::{files, repo};
use crate::{fetch, hash, lockfile::Lock, log::Log};
use aim_android_image::{classpath, redirect, system_server::dex_files};
use std::{fs, path::Path};

const DIR: &str = "image/sync-seam";
const PACKAGE: &str = "com/android/server/content";
const ACCOUNTS: &str = "Lcom/android/server/content/SyncAccounts;";
const AMS: &str = "Lcom/android/server/accounts/AccountManagerService;";

pub fn inputs() -> Vec<std::path::PathBuf> {
    let mut inputs = files(DIR);
    inputs.push(repo("image/original.lock"));
    for name in ["SyncManager", "ContentService"] {
        inputs.push(aim_paths::aosp().join(format!(
            "frameworks/base/services/core/java/{PACKAGE}/{name}.java"
        )));
    }
    for name in ["services.jar", "oat/arm64/services.odex"] {
        inputs.push(
            aim_paths::original_image()
                .join("system/framework")
                .join(name),
        );
    }
    inputs
}

fn pin(path: &Path, expected: &str) -> Result<(), String> {
    let got = hash::sha256_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if got != expected {
        return Err(format!(
            "{}: sha256 {got}, not pinned {expected}",
            path.display()
        ));
    }
    Ok(())
}

fn pair(entry: &str) -> Result<(&str, &str), String> {
    entry
        .split_once('|')
        .ok_or_else(|| format!("bad pin {entry}"))
}

pub fn generate(
    image: &Path,
    work: &Path,
    tools: &Toolchain,
    log: &mut Log,
) -> Result<Vec<u8>, String> {
    let lock = Lock::read(&repo(DIR).join("sources.lock"))?;
    let image_lock = Lock::read(&repo("image/original.lock"))?;
    if aim_android_image::identity::original_identity(image, None)?.as_deref()
        != Some(image_lock.get("SHA256")?)
    {
        return Err("sync seam: original image identity mismatch".into());
    }
    for entry in lock.array("SOURCES") {
        fetch::Source::parse(entry, lock.get("AOSP_TAG")?)?.file(log)?;
    }
    for entry in lock.array("IMAGE") {
        let (name, sha) = pair(entry)?;
        pin(&image.join(name), sha)?;
    }
    let cache = aim_paths::fetched().join("sync-seam-tools");
    fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
    let mut jars = Vec::new();
    for entry in lock.array("TOOLS") {
        let (url, sha) = pair(entry)?;
        let path = cache.join(url.rsplit('/').next().ok_or("tool URL")?);
        if !path.exists() {
            fetch::download(url, &path, log, |_, _| true)?;
        }
        pin(&path, sha)?;
        jars.push(path);
    }
    let cp = std::env::join_paths(jars).map_err(|e| e.to_string())?;
    let smali = |main: &str| {
        let mut cmd = tools.java();
        cmd.arg("-cp")
            .arg(&cp)
            .arg(format!("com.android.tools.smali.{main}.Main"));
        cmd
    };
    let original =
        fs::read(image.join("system/framework/services.jar")).map_err(|e| e.to_string())?;
    let dex = work.join("original.dex");
    fs::write(&dex, dex_files(&original)?[0]).map_err(|e| e.to_string())?;
    let source = work.join("smali");
    log.run(smali("baksmali").arg("d").arg(&dex).arg("-o").arg(&source))?;
    for entry in lock.array("DEX") {
        let (name, sha) = pair(entry)?;
        pin(
            &source
                .join(PACKAGE)
                .join(format!("{}.smali", name.replace('.', "$"))),
            sha,
        )?;
    }
    log.run(
        std::process::Command::new("patch")
            .args(["--batch", "--forward", "--fuzz=0", "-p1", "-d"])
            .arg(&source)
            .arg("-i")
            .arg(repo(DIR).join("seam.patch")),
    )?;
    accounts(&source.join(PACKAGE))?;
    // Link only the affected class cohort against the original image, so
    // unrelated original references are not claimed as part of this exception.
    let cohort = work.join("cohort");
    fs::create_dir_all(&cohort).map_err(|e| e.to_string())?;
    for name in [
        "SyncManager",
        "SyncManager$SyncHandler",
        "ContentService",
        "SyncAccounts",
        "OriginalSyncAccounts",
    ] {
        fs::copy(
            source.join(PACKAGE).join(format!("{name}.smali")),
            cohort.join(format!("{name}.smali")),
        )
        .map_err(|e| e.to_string())?;
    }
    let contract = work.join("contract.dex");
    log.run(
        smali("smali")
            .arg("a")
            .arg(&cohort)
            .arg("-o")
            .arg(&contract),
    )?;
    let mut jars = classpath::jars(image, "bootclasspath.pb", classpath::BOOTCLASSPATH)?;
    jars.extend(classpath::jars(
        image,
        "systemserverclasspath.pb",
        classpath::SYSTEMSERVERCLASSPATH,
    )?);
    java::check_linkage(image, &jars, None, &contract)?;
    let edited = work.join("edited.dex");
    log.run(smali("smali").arg("a").arg(&source).arg("-o").arg(&edited))?;
    redirect::replace_dex(&original, 0, fs::read(&edited).map_err(|e| e.to_string())?)
}

fn accounts(dir: &Path) -> Result<(), String> {
    let mut interface =
        format!(".class public interface abstract {ACCOUNTS}\n.super Ljava/lang/Object;\n");
    let owner = "Lcom/android/server/content/OriginalSyncAccounts;";
    let mut original = format!(
        ".class final {owner}\n.super Ljava/lang/Object;\n.implements {ACCOUNTS}\n.method constructor <init>()V\n.registers 1\ninvoke-direct {{p0}}, Ljava/lang/Object;-><init>()V\nreturn-void\n.end method\n"
    );
    for (method, args, result) in [
        ("validateAccounts(I)V", ", p1", "return-void"),
        (
            "getAccounts(ILjava/lang/String;)[Landroid/accounts/Account;",
            ", p1, p2",
            "move-result-object v0\nreturn-object v0",
        ),
        (
            "getRunningAccountsForSystem()[Landroid/accounts/AccountAndUser;",
            "",
            "move-result-object v0\nreturn-object v0",
        ),
        (
            "getAllAccountsForSystemProcess()[Landroid/accounts/AccountAndUser;",
            "",
            "move-result-object v0\nreturn-object v0",
        ),
    ] {
        interface.push_str(&format!(".method public abstract {method}\n.end method\n"));
        original.push_str(&format!(".method public {method}\n.locals 1\ninvoke-static {{}}, {AMS}->getSingleton(){AMS}\nmove-result-object v0\ninvoke-virtual {{v0{args}}}, {AMS}->{method}\n{result}\n.end method\n"));
    }
    fs::write(dir.join("SyncAccounts.smali"), interface).map_err(|e| e.to_string())?;
    fs::write(dir.join("OriginalSyncAccounts.smali"), original).map_err(|e| e.to_string())
}

/// OatDexFile location, dex magic/version, ZIP location CRC and SHA-1
/// (ART runtime/oat/oat_file.cc). Reject old preopts beside the edited jar.
pub fn check_oat(jar: &[u8], oat: &[u8]) -> Result<(), String> {
    for (i, dex) in dex_files(jar)?.iter().enumerate() {
        let suffix = if i == 0 {
            String::new()
        } else {
            format!("!classes{}.dex", i + 1)
        };
        let location = format!("/system/framework/services.jar{suffix}");
        let mut record = (location.len() as u32).to_le_bytes().to_vec();
        record.extend(location.as_bytes());
        record.extend(&dex[..8]);
        let mut identity = crc32fast::hash(dex).to_le_bytes().to_vec();
        identity.extend(&dex[12..32]);
        let records: Vec<_> = oat
            .windows(record.len())
            .enumerate()
            .filter(|(_, b)| *b == record)
            .collect();
        if records.len() != 1
            || oat.get(records[0].0 + record.len()..records[0].0 + record.len() + identity.len())
                != Some(identity.as_slice())
        {
            return Err(format!("{location}: services.jar/oat mismatch"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_android_image::dex::{self, Dex};

    fn calls(data: &[u8], class: &str, name: &str) -> Vec<(u16, String)> {
        let dex = Dex::parse(data).unwrap();
        let class = dex.class(&format!("L{PACKAGE}/{class};")).unwrap();
        let mut out = Vec::new();
        for code in dex.methods_named(class, name).unwrap() {
            let units = dex::units(data, &code).unwrap();
            let mut pc = 0;
            while pc < units.len() {
                let op = units[pc] & 0xff;
                if matches!(op, 0x6e..=0x72 | 0x74..=0x78) {
                    let (owner, method, sig) = dex.method(units[pc + 1].into()).unwrap();
                    out.push((op, format!("{owner}->{method}{sig}")));
                }
                pc += dex::instruction_units(&units, pc).unwrap();
            }
        }
        out
    }

    #[test]
    fn sync_seam_pinned_image_linkage_and_routes() {
        let Some(image) = aim_paths::original_image_with("system/framework/services.jar") else {
            return;
        };
        let work = aim_paths::scratch_dir("sync-seam");
        let mut log = Log::create(work.join("test.log"), false).unwrap();
        let tools = Toolchain::fetch(&mut log).unwrap();
        assert!(
            generate(&work, &work, &tools, &mut log)
                .unwrap_err()
                .contains("identity mismatch")
        );
        let jar = generate(&image, &work, &tools, &mut log).unwrap();
        let path = work.join("services.jar");
        fs::write(&path, &jar).unwrap();
        tools.verify_dex(&mut log, &path).unwrap();
        let original = fs::read(image.join("system/framework/services.jar")).unwrap();
        assert_eq!(
            &dex_files(&jar).unwrap()[1..],
            &dex_files(&original).unwrap()[1..]
        );
        let data = dex_files(&jar).unwrap()[0];
        let oat = fs::read(image.join("system/framework/oat/arm64/services.odex")).unwrap();
        check_oat(&original, &oat).unwrap();
        assert!(
            check_oat(&jar, &oat)
                .unwrap_err()
                .contains("jar/oat mismatch")
        );
        let mut account_calls = Vec::new();
        for (class, method, count) in [
            ("SyncManager", "removeStaleAccounts", 1),
            ("SyncManager", "onUserUnlocked", 2),
            ("SyncManager", "dumpSyncState", 1),
            ("SyncManager$SyncHandler", "updateRunningAccountsH", 2),
        ] {
            let calls = calls(data, class, method);
            assert!(!calls.iter().any(|(_, call)| call.starts_with(AMS)));
            let injected: Vec<_> = calls
                .into_iter()
                .filter(|(_, call)| call.starts_with(ACCOUNTS))
                .collect();
            assert_eq!(injected.len(), count, "{class}.{method}");
            assert!(injected.iter().all(|(op, _)| *op == 0x72));
            account_calls.extend(injected);
        }
        for (method, count) in [
            ("validateAccounts", 1),
            ("getAccounts", 2),
            ("getRunningAccountsForSystem", 1),
            ("getAllAccountsForSystemProcess", 2),
        ] {
            assert_eq!(
                account_calls
                    .iter()
                    .filter(|(_, call)| call.contains(&format!("->{method}(")))
                    .count(),
                count
            );
            assert!(
                calls(data, "OriginalSyncAccounts", method)
                    .iter()
                    .any(|(_, call)| call == &format!("{AMS}->getSingleton(){AMS}"))
            );
        }
        for class in ["SyncManager", "ContentService"] {
            let constructors = calls(data, class, "<init>");
            assert_eq!(
                constructors
                    .iter()
                    .filter(|(_, call)| call.contains("OriginalSyncAccounts;-><init>"))
                    .count(),
                1
            );
            assert!(
                constructors
                    .iter()
                    .any(|(_, call)| call.ends_with(&format!("Z{ACCOUNTS})V")))
            );
            assert!(
                constructors
                    .iter()
                    .any(|(_, call)| call.contains("Objects;->requireNonNull"))
            );
        }
        assert!(
            calls(data, "ContentService", "getSyncManager")
                .iter()
                .any(|(_, call)| call.ends_with(&format!("Z{ACCOUNTS})V")))
        );
        let replay = calls(data, "SyncManager", "replaySyncState");
        assert!(replay[0].1.ends_with("->onUserUnlocked(I)V"));
        assert!(replay[1].1.ends_with("->post(Ljava/lang/Runnable;)Z"));
        assert!(
            calls(data, "ContentService", "replaySyncState")
                .iter()
                .any(|(_, call)| call.ends_with("->replaySyncState(ILjava/lang/Runnable;)V"))
        );
        let notify = calls(data, "ContentService", "scheduleLocalSyncFromNotify");
        assert!(
            notify
                .iter()
                .any(|(_, call)| call.contains("Binder;->getCallingUid"))
        );
        assert!(
            notify
                .iter()
                .any(|(_, call)| call.contains("->getSyncExemptionForCaller"))
        );
        assert!(
            notify
                .iter()
                .any(|(_, call)| call.contains("->scheduleLocalSync("))
        );
        fs::remove_dir_all(work).unwrap();
    }

    #[test]
    fn sync_seam_rejects_source_image_and_oat_drift() {
        let lock = Lock::read(&repo(DIR).join("sources.lock")).unwrap();
        let work = aim_paths::scratch_dir("sync-seam-pins");
        let mut log = Log::create(work.join("test.log"), false).unwrap();
        for entry in lock.array("SOURCES") {
            let mut source = fetch::Source::parse(entry, lock.get("AOSP_TAG").unwrap()).unwrap();
            source.sha256 = "0".repeat(64);
            assert!(
                source
                    .file(&mut log)
                    .unwrap_err()
                    .contains("sha256 mismatch")
            );
        }
        let changed = work.join("changed");
        fs::write(&changed, "mismatched input").unwrap();
        for entry in lock.array("IMAGE").iter().chain(lock.array("DEX")) {
            assert!(
                pin(&changed, pair(entry).unwrap().1)
                    .unwrap_err()
                    .contains("not pinned")
            );
        }
        fs::remove_dir_all(work).unwrap();
    }
}
