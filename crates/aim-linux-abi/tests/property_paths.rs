//! Actual original property clients across short and deep host socket paths.
use aim_android_init::{
    ImageRoot,
    props::{
        Ucred,
        area::PropAreaReader,
        load::{
            PropertyInitOptions, create_serialized_property_info, property_init,
            start_property_service,
        },
        protocol::{PROP_SUCCESS, serve},
    },
};
use aim_guest_init::{
    identity::Identity,
    paths::Layout,
    props::mapped_properties,
    propsvc::{PropertyEvent, PropertySockets},
};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    sync::mpsc::channel,
    time::{Duration, Instant},
};

struct OwnedChild(Option<Child>);
impl OwnedChild {
    fn id(&self) -> u32 {
        self.0.as_ref().unwrap().id()
    }
    fn stop(mut self) -> Output {
        let child = self.0.as_mut().unwrap();
        if child.try_wait().unwrap().is_none() {
            child.kill().unwrap();
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
    fn finish(mut self) -> Output {
        let child = self.0.as_mut().unwrap();
        let end = Instant::now() + Duration::from_secs(30);
        while child.try_wait().unwrap().is_none() {
            assert!(
                Instant::now() < end,
                "owned original property client timed out"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        self.0.take().unwrap().wait_with_output().unwrap()
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn sha(path: &Path) -> String {
    format!("{:x}", Sha256::digest(fs::read(path).unwrap()))
}
fn command(runtime: &Path, layout: &Layout, args: &[&str]) -> OwnedChild {
    let child = Command::new(runtime)
        .arg("--root")
        .arg(&layout.image)
        .arg("--path-map")
        .arg(layout.path_map_file())
        .arg("--cache")
        .arg(layout.runtime.join("translation-cache"))
        .arg("--by-pid")
        .arg(layout.identity_dir().join("by-pid"))
        .arg("--identity-text")
        .arg("uid\t2000\ngid\t2000\nseclabel\tu:r:shell:s0\n")
        .arg("--guest-fds")
        .arg("0,1,2")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    OwnedChild(Some(child))
}
fn success(output: Output) -> String {
    assert!(
        output.status.success(),
        "original client failed: {:?}\n{}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}
#[test]
#[ignore = "requires pinned original Android image, NDK and explicitly selected immutable runtime"]
fn original_property_clients_share_short_and_deep_native_owners() {
    let inputs = aim_paths::input(
        aim_paths::out().join("property-path-proof.inputs"),
        "immutable runtime input receipt",
    )
    .expect("NOT RUN: missing explicit property-path proof inputs");
    let text = fs::read_to_string(inputs).unwrap();
    let field = |name: &str| {
        text.lines()
            .find_map(|line| {
                line.split_once('\t')
                    .filter(|(key, _)| *key == name)
                    .map(|(_, value)| value.to_string())
            })
            .expect("required input receipt field")
    };
    let runtime = aim_paths::input(field("runtime").into(), "immutable runtime")
        .expect("NOT RUN: missing runtime");
    let expected_sha = field("runtime_sha256");
    assert_eq!(sha(&runtime), expected_sha);
    let image = aim_paths::original_image_with("system/bin/setprop")
        .expect("NOT RUN: missing original setprop");
    assert!(
        image.join("system/bin/getprop").is_file(),
        "NOT RUN: missing original getprop"
    );
    let _lease = aim_storage::system::ImageLease::read_root(&image).unwrap();
    let clang = aim_paths::ndk_clang(35).expect("NOT RUN: missing pinned NDK clang");
    let root = Path::new("/tmp").join(format!(
        "aim-prop-path-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    let scratch = Scratch(root.clone());
    let helper = root.join("serial.c");
    fs::write(&helper,r#"#include <sys/system_properties.h>
#include <stdio.h>
int main(int argc,char **argv){if(argc!=2)return 2;const prop_info *p=__system_property_find(argv[1]);if(!p)return 3;printf("%u %u\n",__system_property_serial(p),__system_property_area_serial());return 0;}
"#).unwrap();
    let compiled = Command::new(clang)
        .args(["-O2", "-fPIE", "-pie"])
        .arg(&helper)
        .arg("-o")
        .arg(root.join("serial"))
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    for deep in [false, true] {
        let mut case = root.join(if deep { "deep" } else { "short" });
        if deep {
            for _ in 0..5 {
                case = case.join("property-transport-owned-deep-directory");
            }
        }
        let layout = Layout::new(image.clone(), case.join("data"), Some(case.join("run")));
        layout.prepare().unwrap();
        fs::create_dir_all(layout.identity_dir().join("by-pid")).unwrap();
        fs::write(layout.path_map_file(), layout.path_map().to_file_text()).unwrap();
        fs::copy(root.join("serial"), layout.data.join("data/serial")).unwrap();
        let socket = layout.socket_dir().join("property_service");
        let length = socket.as_os_str().as_encoded_bytes().len();
        assert_eq!(length >= 104, deep);
        let mut diagnostics = Vec::new();
        let original = ImageRoot::new(&image);
        let vendor = aim_android_init::rc::vendor_android_version(&original)
            .expect("pinned original vendor API");
        let info = create_serialized_property_info(&original, vendor, &mut diagnostics).unwrap();
        let mut props = mapped_properties(&layout.properties_dir(), info).unwrap();
        property_init(
            &mut props,
            &original,
            &PropertyInitOptions {
                vendor_api_level: vendor,
                ..Default::default()
            },
        );
        start_property_service(&mut props);
        let (events, requests) = channel();
        let sockets = PropertySockets::start(&layout.socket_dir(), events).unwrap();
        let name = "debug.aim.property_path";
        let value = if deep {
            "deep-original"
        } else {
            "short-original"
        };
        let child = command(&runtime, &layout, &["/system/bin/setprop", name, value]);
        let pid = child.id();
        let request = match requests.recv_timeout(Duration::from_secs(10)) {
            Ok(PropertyEvent::Set(request)) => request,
            other => {
                let output = child.stop();
                panic!(
                    "actual property request {other:?}: status={:?} stdout={} stderr={}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        };
        assert_eq!(request.peer_pid, pid as i32);
        assert_eq!(request.peer_host_uid, unsafe { libc::geteuid() });
        let identity =
            Identity::read_entry(&layout.identity_dir().join("by-pid").join(pid.to_string()))
                .expect("actual child identity publication");
        assert_eq!(identity.uid, 2000);
        assert_eq!(identity.seclabel, "u:r:shell:s0");
        let result = serve(
            &mut props,
            &request.request,
            Some(&identity.seclabel),
            &Ucred {
                pid: request.peer_pid,
                uid: identity.uid,
                gid: identity.gid,
            },
        );
        assert_eq!(result.reply, Some(PROP_SUCCESS));
        request.reply(result.reply.unwrap());
        success(child.finish());
        let context = props
            .areas()
            .info_area()
            .property_info(name)
            .0
            .unwrap()
            .to_string();
        let file = layout.properties_dir().join(context);
        let bytes = fs::read(&file).unwrap();
        let property = PropAreaReader::new(&bytes).unwrap().get(name).unwrap();
        assert_eq!(property.value, value);
        let serial = fs::read(layout.properties_dir().join("properties_serial")).unwrap();
        let serial = u32::from_le_bytes(serial[4..8].try_into().unwrap());
        assert_eq!(
            success(command(&runtime, &layout, &["/system/bin/getprop", name]).finish()).trim(),
            value
        );
        let guest_serial = success(command(&runtime, &layout, &["/data/serial", name]).finish());
        assert_eq!(
            guest_serial.trim(),
            format!("{} {}", property.serial, serial)
        );
        assert_eq!(
            fs::read(&file).unwrap(),
            bytes,
            "original readers must not reset owner mappings"
        );
        eprintln!(
            "PROPERTY_PATH_PASS deep={deep} bytes={length} peer={pid} serial={} global={serial} value={value}",
            property.serial
        );
        drop(sockets);
        drop(props);
        assert_eq!(
            fs::read(&file).unwrap(),
            bytes,
            "socket teardown must preserve mapped values"
        );
    }
    assert_eq!(sha(&runtime), expected_sha);
    drop(scratch);
}
