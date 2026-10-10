//! Official UsesSdkTest constant fixtures compiled without changing the CTS JAR.
use aim_services::package::parse::{self,Error,Platform};
use std::{fs,path::PathBuf,process::Command};
struct Data(PathBuf);
impl Drop for Data {fn drop(&mut self){fs::remove_dir_all(&self.0).unwrap();}}
fn data()->Data {
    let path=std::env::temp_dir().join(format!("aim-extension-sdk-{}-{}",std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    fs::create_dir(&path).unwrap();Data(path)
}
fn compile(data:&Data,name:&str,manifest:&str)->PathBuf {
    let manifest=manifest.replacen("<manifest ", &format!("<manifest package=\"fixture.{name}\" "),1);
    let source=data.0.join(format!("{name}.xml"));fs::write(&source,manifest).unwrap();
    let apk=data.0.join(format!("{name}.apk"));
    let output=Command::new(aim_paths::fetched().join("java/build-tools-36.0.0/android-16/aapt2"))
        .arg("link").arg("--manifest").arg(source)
        .arg("-I").arg(aim_paths::derived_image().join("system/framework/framework-res.apk"))
        .arg("-o").arg(&apk).output().unwrap();
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));apk
}
fn platform()->Platform {
    let mut platform=Platform::load(&aim_paths::derived_image(),Default::default()).unwrap();
    // This fixture supplies the original absent-property default (0), sufficient
    // for CTS's declared minimum 0. Production receives activated runtime values.
    platform.set_sdk_extensions(&|_|Ok(None)).unwrap();platform
}
fn official_manifests()->String {
    let jar=aim_paths::fetched().join("cts-tradefed/android-cts/testcases/CtsPackageManagerParsingHostTestCases/CtsPackageManagerParsingHostTestCases.jar");
    assert!(jar.is_file(),"official CTS parsing JAR is required");
    let output=Command::new("javap").args(["-c","-p","-classpath"]).arg(jar)
        .arg("android.content.pm.parsing.cts.host.UsesSdkTest").output().expect("JDK javap required");
    assert!(output.status.success(),"{}",String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}
fn manifest(bytecode:&str,method:&str)->String {
    let body=bytecode.split(&format!("public final void {method}();")).nth(1).unwrap()
        .split("public final void ").next().unwrap();
    let text=body.lines().find_map(|line|line.split_once("// String ").map(|(_,value)|value))
        .filter(|value|value.contains("<manifest")).unwrap();
    // javap escapes the original generated test literal's backslashes and quotes.
    text.replace("\\\\n","\n").replace("\\\\\"","\"").replace("\\n","\n").replace("\\\"","\"")
}
#[test]
fn official_uses_sdk_last_extension_and_last_tag_reset() {
    let data=data();let bytecode=official_manifests();let platform=platform();
    for (method,expected) in [("takeLastExtensionSdk",Some(vec![(31,0)])),
        ("lastDeclarationOverridesAllPrevious",None),("emptyUsesSdk",None),
        ("takeLastAfterAppTag",Some(vec![(31,0)]))] {
        let apk=compile(&data,&method.to_lowercase(),&manifest(&bytecode,method));
        let parsed=parse::parse(&apk,"/data/app/extension-fixture/base.apk",0,&platform).unwrap();
        assert_eq!(parsed.min_extension_versions,expected,"{method}");
    }
}
#[test]
fn extension_sdk_duplicate_validation_actual_property_policy_and_errors() {
    let data=data();let mut platform=platform();
    platform.set_sdk_extensions(&|name|Ok((name=="build.version.extensions.r").then(||"2".into()))).unwrap();
    let xml=|children:&str|format!("<manifest xmlns:android=\"http://schemas.android.com/apk/res/android\"><uses-sdk android:targetSdkVersion=\"29\">{children}</uses-sdk><application/></manifest>");
    let duplicate=compile(&data,"duplicate",&xml("<extension-sdk android:sdkVersion=\"30\" android:minExtensionVersion=\"1\"/><extension-sdk android:sdkVersion=\"30\" android:minExtensionVersion=\"2\"/><extension-sdk android:sdkVersion=\"32\" android:minExtensionVersion=\"0\"/>"));
    assert_eq!(parse::parse(&duplicate,"/data/app/extension-fixture/base.apk",0,&platform).unwrap().min_extension_versions,Some(vec![(30,2),(32,0)]));
    for (name,children,older) in [("missing_sdk","<extension-sdk android:minExtensionVersion=\"0\"/>",false),
        ("missing_min","<extension-sdk android:sdkVersion=\"30\"/>",false),
        ("invalid_base","<extension-sdk android:sdkVersion=\"29\" android:minExtensionVersion=\"0\"/>",false),
        ("incompatible","<extension-sdk android:sdkVersion=\"30\" android:minExtensionVersion=\"3\"/>",true)] {
        let apk=compile(&data,name,&xml(children));
        let error=parse::parse(&apk,"/data/app/extension-fixture/base.apk",0,&platform).unwrap_err();
        assert_eq!(matches!(error,Error::OlderSdk(_)),older,"{name}: {error:?}");
        if !older{assert!(matches!(error,Error::Parse(_)),"{name}: {error:?}");}
    }
}
