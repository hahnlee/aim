//! Pinned parser/formatter oracle over production Java shell callbacks.
//! Android runtime boundaries and the scoped PM route are doubled; no guest boot.
use std::{fs, process::Command};
mod common {
    pub mod java;
    pub mod runtime;
}
#[test]
#[ignore = "requires pinned JDK and network; no guest boot"]
fn domain_shell_output_matches_android16() {
    let data = common::runtime::Data(
        std::env::temp_dir().join(format!("aim-domain-shell-{}", std::process::id())),
    );
    fs::create_dir(&data.0).unwrap();
    let jdk =
        aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin");
    let stubs = data.0.join("stubs");
    fs::create_dir(&stubs).unwrap();
    common::runtime::run(
        Command::new(jdk.join("javac"))
            .args(["--release", "17", "-d"])
            .arg(&stubs)
            .args(common::java::sources(
                &aim_paths::root().join("java/device-services/stubs"),
            )),
    );
    let mut sources = Vec::new();
    for (name, url, hash) in [
        (
            "DomainVerificationShell",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/DomainVerificationShell.java",
            "f205a6ca250bb8d956774c6504940e6e200e75cda42abcdb44d08499d628579f",
        ),
        (
            "BasicShellCommandHandler",
            "platform/frameworks/libs/modules-utils/+/android-16.0.0_r1/java/com/android/modules/utils/BasicShellCommandHandler.java",
            "f56b4ac8f89b0361e31d63790357fc108e6d3d65b4b0338c67ca6c62e8f48bf9",
        ),
        (
            "DomainVerificationDebug",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/DomainVerificationDebug.java",
            "15653491f3ce9a2bd4bf7ce9a894282e5e700d5e9693f8665ff547917e48963a",
        ),
        (
            "IndentingPrintWriter",
            "platform/frameworks/base/+/android-16.0.0_r1/core/java/android/util/IndentingPrintWriter.java",
            "539d9d74149dc9f0d2ed61da9d25dd959bb90624977988123f594441976409ad",
        ),
        (
            "DomainVerificationState",
            "platform/frameworks/base/+/android-16.0.0_r1/core/java/android/content/pm/verify/domain/DomainVerificationState.java",
            "dc7c823b0fd513ad0c8f5394d6a5a770fea4275731e861d7c4438e4ba301362d",
        ),
        (
            "DomainVerificationManagerInternal",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/DomainVerificationManagerInternal.java",
            "b0bc7ed12b18934622cac52024c79fd627527f53c345d2a74809c50a4d1d275a",
        ),
        (
            "DomainVerificationEnforcer",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/DomainVerificationEnforcer.java",
            "d5f02086fa3b148344f81f6b5d09c15b2b5702fc0efb17bac62dd5cea0ff7f1a",
        ),
        (
            "PackageManagerShellCommand",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/PackageManagerShellCommand.java",
            "96c8109b2c84abdc0cf145e5bbf88751375cf3c25a6b4bd5fb719055370a1387",
        ),
    ] {
        let fetched = common::runtime::run_with_status_receipt(Command::new("curl").args([
            "-fsSL",
            &format!("https://android.googlesource.com/{url}?format=TEXT"),
        ]));
        let encoded = data.0.join("source.base64");
        fs::write(&encoded, fetched.stdout).unwrap();
        let decoded = common::runtime::run_with_status_receipt(
            Command::new("base64").arg("-d").arg("-i").arg(&encoded),
        );
        let path = data.0.join(format!("{name}.java"));
        fs::write(&path, &decoded.stdout).unwrap();
        let digest = common::runtime::run_with_status_receipt(
            Command::new("shasum").args(["-a", "256"]).arg(&path),
        );
        assert!(String::from_utf8(digest.stdout).unwrap().starts_with(hash));
        if name == "PackageManagerShellCommand" {
            let original = String::from_utf8_lossy(&decoded.stdout);
            assert!(original.contains("mDomainVerificationShell.runCommand(this, cmd)"));
            assert!(original.contains("return domainVerificationResult ? 0 : 1;"));
            continue;
        }
        if name == "DomainVerificationEnforcer" {
            let original = String::from_utf8_lossy(&decoded.stdout);
            let method = &original[original.find("    public void assertInternal").unwrap()..];
            let method = &method[..method.find("\n    /**").unwrap()];
            // Execute the exact permission method, doubling unrelated Context queries.
            fs::write(&path, format!("package com.android.server.pm.verify.domain; import android.os.Process; public class DomainVerificationEnforcer {{ public DomainVerificationEnforcer(android.content.Context c){{}} public void assertApprovedQuerent(int uid, com.android.server.pm.verify.domain.proxy.DomainVerificationProxy p){{assertInternal(uid);}} {method} }}")).unwrap();
        }
        if name == "DomainVerificationManagerInternal" {
            // Keep the pinned approval labels, without compiling unrelated internal APIs.
            let original = String::from_utf8_lossy(&decoded.stdout);
            let method = &original[original
                .find("    static String approvalLevelToDebugString")
                .unwrap()..];
            let method = &method[..method.find("\n    /**").unwrap()];
            let constants = &original[original
                .find("    int APPROVAL_LEVEL_NOT_INSTALLED")
                .unwrap()..];
            let constants = &constants[..constants.find("    @IntDef").unwrap()];
            fs::write(&path, format!("package com.android.server.pm.verify.domain; public interface DomainVerificationManagerInternal {{ @interface ApprovalLevel {{}} {constants} {method} }}")).unwrap();
        }
        sources.push(path);
    }
    let fixture = aim_paths::root().join("crates/aim-services/tests/fixtures/domain_shell");
    sources.extend(common::java::sources(&fixture));
    for name in ["NonNull", "Nullable", "UserIdInt"] {
        let path = data.0.join(format!("{name}.java"));
        fs::write(
            &path,
            format!("package android.annotation; public @interface {name} {{}}"),
        )
        .unwrap();
        sources.push(path);
    }
    sources.push(aim_paths::root().join(
        "java/device-services/src/com/android/server/pm/verify/domain/AppHostDomainShell.java",
    ));
    sources.push(
        aim_paths::root()
            .join("java/device-services/src/com/android/server/pm/NativePackageReadCommand.java"),
    );
    common::java::production_classes(&data.0, &jdk.parent().unwrap(), &stubs);
    let classes = data.0.join("classes");
    fs::create_dir(&classes).unwrap();
    common::runtime::run(
        Command::new(jdk.join("javac"))
            .args(["--release", "17", "-cp"])
            .arg(&stubs)
            .arg("-d")
            .arg(&classes)
            .args(sources),
    );
    for (args, expected) in [
        (
            vec!["get-app-links", "--invalid"],
            "Error: unknown option: --invalid\n",
        ),
        (
            vec!["get-app-link-owners"],
            "Error: no package name or domain specified\n",
        ),
    ] {
        let result = Command::new(jdk.join("java"))
            .arg("-cp")
            .arg(format!("{}:{}", classes.display(), stubs.display()))
            .arg("DomainShellTest")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(1));
        assert!(result.stdout.is_empty());
        assert_eq!(String::from_utf8(result.stderr).unwrap(), expected);
    }
    common::runtime::run(
        Command::new(jdk.join("java"))
            .arg("-cp")
            .arg(format!("{}:{}", classes.display(), stubs.display()))
            .arg("DomainShellTest"),
    );
}
