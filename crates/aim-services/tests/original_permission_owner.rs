//! Genuine original AMS permission decisions with registered instrumentation delegation.
use std::{fs, path::{Path,PathBuf}, process::{Command,Stdio}, time::{Duration,Instant}};
mod common { pub mod java; pub mod runtime; }
use common::runtime::{Boot,Data};
struct OwnedChild(Option<std::process::Child>);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let Some(child)=self.0.as_mut() else{return};
        if matches!(child.try_wait(),Ok(Some(_))) { let _=child.wait(); return; }
        unsafe { libc::kill(child.id() as i32,libc::SIGTERM); }
        let grace=Instant::now()+Duration::from_secs(1);
        while Instant::now()<grace {
            if matches!(child.try_wait(),Ok(Some(_))) { let _=child.wait(); return; }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _=child.kill(); let _=child.wait();
    }
}
fn output(path:&Path)->std::io::Result<String>{
    use std::io::Read;
    let mut bytes=Vec::new();fs::File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len()>65536{return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"fixture output exceeds64KiB"));}
    String::from_utf8(bytes).map_err(|e|std::io::Error::new(std::io::ErrorKind::InvalidData,e))
}
fn captured(command:&mut Command,dir:&Path,label:&str,limit:Duration)->String {
    let out=dir.join(format!("{label}.stdout"));let err=dir.join(format!("{label}.stderr"));
    let child=command.stdin(Stdio::null()).stdout(fs::File::create(&out).unwrap()).stderr(fs::File::create(&err).unwrap()).spawn().unwrap();
    let mut owned=OwnedChild(Some(child));let pid=owned.0.as_ref().unwrap().id();
    let deadline=Instant::now()+limit;
    let status=loop{
        if let Some(status)=owned.0.as_mut().unwrap().try_wait().expect("owned child status") {break status;}
        assert!(Instant::now()<deadline,"{label}: owned PID{pid} exceeded {limit:?}");
        std::thread::sleep(Duration::from_millis(10));
    };
    owned.0.as_mut().unwrap().wait().expect("owned child reap");
    let stdout=output(&out);let stderr=output(&err);
    assert!(status.success(),"{label}: actual PID{pid} {status}; stdout:{stdout:?}; stderr:{stderr:?}");
    let stdout=stdout.expect("bounded stdout capture");stderr.expect("bounded stderr capture");
    stdout
}
fn build(dir:&Path,image:&Path)->PathBuf {
    let java=aim_paths::fetched().join("java");let jdk=java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");let tools=java.join("build-tools-36.0.0/android-16");
    let android=aim_paths::sdk().unwrap().join("platforms/android-36/android.jar");assert!(android.is_file(),"NOT RUN: pinned API36 android.jar missing");
    let api=dir.join("api/android/app");let stubs=dir.join("stubs");let classes=dir.join("classes");let dex=dir.join("dex");for path in [&api,&stubs,&classes,&dex]{fs::create_dir_all(path).unwrap();}
    fs::write(api.join("ActivityManager.java"),"package android.app; public final class ActivityManager { public static IActivityManager getService(){throw new RuntimeException(\"compile-only\");} }").unwrap();
    fs::write(api.join("IActivityManager.java"),"package android.app; public interface IActivityManager { int checkPermission(String name,int pid,int uid) throws android.os.RemoteException; }").unwrap();
    captured(Command::new(jdk.join("bin/javac")).args(["--release","17","-classpath"]).arg(&android).arg("-d").arg(&stubs).args(common::java::sources(&dir.join("api"))),dir,"api-compile",Duration::from_secs(30));
    let fixture=aim_paths::root().join("crates/aim-services/tests/fixtures/permission");
    captured(Command::new(jdk.join("bin/javac")).args(["--release","17","-classpath"]).arg(format!("{}:{}",stubs.display(),android.display())).arg("-d").arg(&classes).arg(fixture.join("OriginalPermissionInstrumentation.java")),dir,"fixture-compile",Duration::from_secs(30));
    fn class_files(dir:&Path,out:&mut Vec<PathBuf>){for entry in fs::read_dir(dir).unwrap(){let path=entry.unwrap().path();if path.is_dir(){class_files(&path,out)}else if path.extension().is_some_and(|s|s=="class"){out.push(path)}}}
    let mut files=Vec::new();class_files(&classes,&mut files);
    captured(Command::new(jdk.join("bin/java")).arg("-cp").arg(tools.join("lib/d8.jar")).args(["com.android.tools.r8.D8","--release","--min-api","36","--lib"]).arg(&android).arg("--classpath").arg(&stubs).arg("--output").arg(&dex).args(files),dir,"d8",Duration::from_secs(30));
    use aim_android_image::{classpath::{self,BOOTCLASSPATH},linkage::ClassPath};
    let jars=classpath::jars(image,"bootclasspath.pb",BOOTCLASSPATH).unwrap();let missing=ClassPath::read(image,&jars).unwrap().unresolved(&fs::read(dex.join("classes.dex")).unwrap()).unwrap();assert!(missing.is_empty(),"original framework linkage {missing:?}");
    let unsigned=dir.join("unsigned.apk");let apk=dir.join("permission.apk");let key=dir.join("fixture.keystore");
    captured(Command::new(tools.join("aapt2")).args(["link","--manifest"]).arg(fixture.join("AndroidManifest.xml")).arg("-I").arg(image.join("system/framework/framework-res.apk")).arg("-o").arg(&unsigned),dir,"aapt",Duration::from_secs(30));
    captured(Command::new(jdk.join("bin/jar")).arg("uf").arg(&unsigned).arg("-C").arg(&dex).arg("classes.dex"),dir,"add-dex",Duration::from_secs(30));
    captured(Command::new(jdk.join("bin/keytool")).args(["-genkeypair","-alias","fixture","-keyalg","RSA","-keysize","2048","-validity","3650","-dname","CN=AIM original permission fixture","-storepass","fixture-only","-keypass","fixture-only","-keystore"]).arg(&key),dir,"keytool",Duration::from_secs(30));
    captured(Command::new(jdk.join("bin/java")).arg("-jar").arg(tools.join("lib/apksigner.jar")).args(["sign","--ks"]).arg(&key).args(["--ks-pass","pass:fixture-only","--key-pass","pass:fixture-only","--out"]).arg(&apk).arg(&unsigned),dir,"sign",Duration::from_secs(30));
    apk
}
#[test]
#[ignore="host-only: pinned API36 SDK/JDK/build-tools and original image required"]
fn authored_instrumentation_compiles_links_and_packages(){let dir=std::env::temp_dir().join(format!("aim-permission-build-{}",std::process::id()));fs::create_dir(&dir).unwrap();let data=Data(dir);build(&data.0,&aim_paths::original_image());}
#[test]
#[ignore="one fresh Original boot; actual AMS and registered UiAutomation; requires approved explicit cohort"]
fn original_allowed_denied_and_registered_pid_delegation(){
    let inputs=common::runtime::cohort::load().unwrap();inputs.revalidate(true).unwrap();
    let dir=std::env::temp_dir().join(format!("aim-permission-original-{}",std::process::id()));fs::create_dir(&dir).unwrap();let data=Data(dir);let apk=build(&data.0,&inputs.image(common::runtime::cohort::Variant::Original));
    let boot=Boot::new(inputs.tools["aimctl"].path.clone(),data.0.join("guest"));captured(boot.start_command().args(["start","--windows"]),&data.0,"boot-start",Duration::from_secs(270));
    let deadline=Instant::now()+Duration::from_secs(300);let mut n=0;
    loop{let value=captured(boot.command().args(["shell","getprop","sys.boot_completed"]),&data.0,&format!("ready-{n}"),Duration::from_secs(15));if value.trim()=="1"{break}assert!(Instant::now()<deadline,"original boot incomplete");n+=1;std::thread::sleep(Duration::from_secs(1));}
    let guest=boot.data.join("data/local/tmp/permission.apk");fs::copy(&apk,&guest).unwrap();
    let install=captured(boot.command().args(["shell","pm","install","/data/local/tmp/permission.apk"]),&data.0,"install",Duration::from_secs(90));assert!(install.contains("Success"),"{install}");
    let result=captured(boot.command().args(["shell","am","instrument","-w","dev.aim.test.permission/dev.aim.test.permission.OriginalPermissionInstrumentation"]),&data.0,"instrumentation",Duration::from_secs(90));
    assert!(result.contains("INSTRUMENTATION_CODE: -1") && result.contains("permission_proof=allowed,denied,delegated,dropped pid=") && !result.contains("permission_failure"),"{result}");
    let removed=captured(boot.command().args(["shell","pm","uninstall","dev.aim.test.permission"]),&data.0,"uninstall",Duration::from_secs(90));assert!(removed.contains("Success"),"{removed}");
}
#[test]
fn helper_unwind_reaps_its_child_and_rejects_oversized_output(){
    let child=Command::new("/bin/sleep").arg("30").spawn().unwrap();let pid=child.id();
    let caught=std::panic::catch_unwind(||{let _owned=OwnedChild(Some(child));panic!("authored capture failure");});
    assert!(caught.is_err());let mut status=0;assert_eq!(unsafe{libc::waitpid(pid as i32,&mut status,libc::WNOHANG)},-1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(),Some(libc::ECHILD));
    let path=std::env::temp_dir().join(format!("aim-permission-output-{}",std::process::id()));
    fs::write(&path,vec![b'x';65537]).unwrap();let result=output(&path);fs::remove_file(&path).unwrap();assert_eq!(result.unwrap_err().kind(),std::io::ErrorKind::InvalidData);
}
