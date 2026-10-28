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
    let mut delivery_sources = Vec::new();
    for (name, url, hash) in [
        (
            "PowerWhitelistManager",
            "platform/frameworks/base/+/android-16.0.0_r1/apex/jobscheduler/framework/java/android/os/PowerWhitelistManager.java",
            "1a8022e659ab77d0827c787068bc16ac95bbb60bbbb0671d7ac6f6589159dd82",
        ),
        (
            "PowerExemptionManager",
            "platform/frameworks/base/+/android-16.0.0_r1/apex/jobscheduler/framework/java/android/os/PowerExemptionManager.java",
            "4cb0d746ed94e9b03e27befb2340e9955aa72c72689b0f633ce49cc328d58fe8",
        ),
        (
            "DomainVerificationProxy",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/proxy/DomainVerificationProxy.java",
            "56feded97000fb9d3df158502e2682de76a35438f9b2ea9fda4e77d62687fff8",
        ),
        (
            "DomainVerificationProxyV2",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/proxy/DomainVerificationProxyV2.java",
            "3cc6d484f2dc920f28d2d7f4dd868467d158ed55389ca2deb3b3bde03bbc8f80",
        ),
        (
            "DomainVerificationProxyUnavailable",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/proxy/DomainVerificationProxyUnavailable.java",
            "56fe613a530712ad1d0ba1cdbf8ad3d6a3529858302ed274499b7bef75c4600d",
        ),
        (
            "DomainVerificationProxyCombined",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/proxy/DomainVerificationProxyCombined.java",
            "c02f05aeb30697600b5a12ccb7504fd32f965470ae0776bbe48f34d5d3f15e2a",
        ),
        (
            "DomainVerificationMessageCodes",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/DomainVerificationMessageCodes.java",
            "ad55426bbeed0438d12e75eef3d8cd833f25e9eccbdc57909459abf0e2890c31",
        ),
        (
            "DomainVerificationRequest",
            "platform/frameworks/base/+/android-16.0.0_r1/core/java/android/content/pm/verify/domain/DomainVerificationRequest.java",
            "7986d9cce83097e9f85521b7d7f88028e7cde62bc66f0e02ac7d598d7aca5389",
        ),
        (
            "DomainVerificationService",
            "platform/frameworks/base/+/android-16.0.0_r1/services/core/java/com/android/server/pm/verify/domain/DomainVerificationService.java",
            "d1c6744e9b4f161cf7002839c4b9a0867af92f37f951c2fe3fca4e3dca5615b3",
        ),
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
        if name == "PowerWhitelistManager" {
            let original = String::from_utf8_lossy(&decoded.stdout);
            assert!(original.contains("PowerExemptionManager.REASON_DOMAIN_VERIFICATION_V2"));
            assert!(original.contains(
                "PowerExemptionManager.TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED"
            ));
            continue;
        }
        if name == "PowerExemptionManager" {
            let original = String::from_utf8_lossy(&decoded.stdout);
            assert!(original.contains("REASON_DOMAIN_VERIFICATION_V2 = 308;"));
            assert!(original.contains("TEMPORARY_ALLOW_LIST_TYPE_FOREGROUND_SERVICE_ALLOWED = 0;"));
            continue;
        }
        if name.starts_with("DomainVerificationProxy")
            || matches!(
                name,
                "DomainVerificationMessageCodes" | "DomainVerificationRequest"
            )
        {
            delivery_sources.push(path);
            continue;
        }
        if name == "DomainVerificationService" {
            let original = String::from_utf8_lossy(&decoded.stdout);
            let mut methods = String::new();
            for signature in [
                "    public void setDomainVerificationStatusInternal(@Nullable String",
                "    private void setDomainVerificationStatusInternal(@NonNull DomainVerificationPkgState",
                "    private void removeUserStatesForDomain(",
                "    public void clearDomainVerificationState(",
                "    private void resetDomainState(",
                "    private boolean applyImmutableState(",
                "    public void clearUserStates(",
                "    public void verifyPackages(",
                "    private void addIfShouldBroadcastLocked(",
                "    private boolean shouldReBroadcastPackage(",
                "    private void sendBroadcast(@NonNull Set<String>",
            ] {
                let method = &original[original.find(signature).unwrap()..];
                let mut depth = 0;
                let start = method.find('{').unwrap();
                let end = method
                    .char_indices()
                    .skip_while(|(i, _)| *i < start)
                    .find_map(|(i, c)| {
                        if c == '{' {
                            depth += 1;
                        }
                        if c == '}' {
                            depth -= 1;
                            if depth == 0 {
                                return Some(i + 1);
                            }
                        }
                        None
                    })
                    .unwrap();
                methods.push_str(&method[..end]);
                methods.push('\n');
            }
            let path = data.0.join("OriginalDomainMutations.java");
            fs::write(&path, format!("package com.android.server.pm.verify.domain; import android.annotation.*; import android.util.*; import android.os.UserHandle; import android.content.pm.PackageManager.NameNotFoundException; import android.content.pm.verify.domain.DomainVerificationState; import java.util.*; public class OriginalDomainMutations extends MutationFixture {{ {methods} }}")).unwrap();
            sources.push(path);
            continue;
        }
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
        delivery_sources.push(path.clone());
        sources.push(path);
    }
    sources.push(aim_paths::root().join(
        "java/device-services/src/com/android/server/pm/verify/domain/AppHostDomainShell.java",
    ));
    sources.push(
        aim_paths::root()
            .join("java/device-services/src/com/android/server/pm/NativePackageReadCommand.java"),
    );
    let production = common::java::production_classes(&data.0, &jdk.parent().unwrap(), &stubs);
    delivery_sources.extend(common::java::sources(
        &aim_paths::root().join("crates/aim-services/tests/fixtures/domain_delivery"),
    ));
    delivery_sources.push(fixture.join("android/util/ArraySet.java"));
    let delivery_classes = data.0.join("delivery-classes");
    fs::create_dir(&delivery_classes).unwrap();
    let delivery_classpath = format!("{}:{}", production.display(), stubs.display());
    common::runtime::run(
        Command::new(jdk.join("javac"))
            .args(["--release", "17", "-cp"])
            .arg(&delivery_classpath)
            .arg("-d")
            .arg(&delivery_classes)
            .args(delivery_sources),
    );
    let delivery = common::runtime::run_with_status_receipt(
        Command::new(jdk.join("java"))
            .arg("-cp")
            .arg(format!(
                "{}:{delivery_classpath}",
                delivery_classes.display()
            ))
            .arg("DomainDeliveryTest"),
    );
    assert!(delivery.stderr.is_empty());
    assert_eq!(
        String::from_utf8(delivery.stdout).unwrap(),
        "domain delivery: selection, original V2 request/broadcast, unavailable verifier, authorization and failures passed\n"
    );
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
    common::runtime::run(
        Command::new(jdk.join("java"))
            .arg("-cp")
            .arg(format!("{}:{}", classes.display(), stubs.display()))
            .arg("com.android.server.pm.verify.domain.MutationFixture"),
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
