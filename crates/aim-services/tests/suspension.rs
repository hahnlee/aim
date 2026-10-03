//! Compare the native XML owner with the pinned original SuspendDialogInfo.
use aim_android_xml::{Element, Value};
use aim_services::package::restrictions::dialog::DialogInfo;
use std::{
    fs,
    process::Command,
    time::{Duration, Instant},
};
mod common {
    pub mod java;
    pub mod runtime;
}
use common::runtime::{Boot, Data, run};

#[test]
#[ignore = "requires pinned image, aimctl, JDK and d8; run explicitly"]
fn suspension_parameters_match_original_xml_owners() {
    let data = Data(std::env::temp_dir().join(format!("aim-suspend-{}", std::process::id())));
    fs::create_dir(&data.0).unwrap();
    let java = aim_paths::fetched().join("java");
    let jdk = java.join("temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home");
    let classes = data.0.join("classes");
    let stubs = data.0.join("stubs");
    let dex = data.0.join("dex");
    for path in [&classes, &stubs, &dex] {
        fs::create_dir(path).unwrap();
    }
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&stubs)
        .args(common::java::sources(
            &aim_paths::root().join("java/device-services/stubs"),
        )));
    run(Command::new(jdk.join("bin/javac"))
        .args(["--release", "17", "-d"])
        .arg(&classes)
        .arg("-classpath")
        .arg(&stubs)
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/SuspensionDialogOracle.java"),
        )
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/PersistableBundleOracle.java"),
        ));
    run(Command::new(jdk.join("bin/java"))
        .arg("-cp")
        .arg(java.join("build-tools-36.0.0/android-16/lib/d8.jar"))
        .args([
            "com.android.tools.r8.D8",
            "--release",
            "--min-api",
            "36",
            "--lib",
        ])
        .arg(&jdk)
        .arg("--classpath")
        .arg(&stubs)
        .arg("--output")
        .arg(&dex)
        .arg(classes.join("SuspensionDialogOracle.class"))
        .arg(classes.join("PersistableBundleOracle.class")));
    common::java::check_linkage(
        &dex.join("classes.dex"),
        &["/system/framework/services.jar"],
    )
    .unwrap();
    let boot = Boot {
        ctl: aim_paths::root().join("target/release/aimctl"),
        data: data.0.join("guest"),
    };
    run(boot.command().args(["start", "--windows"]));
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let result = boot
            .command()
            .args(["shell", "getprop", "sys.boot_completed"])
            .output()
            .unwrap();
        if result.status.success() && String::from_utf8_lossy(&result.stdout).trim() == "1" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "disposable boot did not complete"
        );
        std::thread::sleep(Duration::from_secs(1));
    }
    let directory = boot.data.join("data/local/tmp/suspension-dialogs");
    fs::create_dir(&directory).unwrap();
    fs::copy(dex.join("classes.dex"), directory.join("oracle.dex")).unwrap();
    let mut inputs = vec![aim_android_xml::read(b"<dialog-info/>").unwrap()];
    for field in [
        "iconResId",
        "titleResId",
        "buttonTextResId",
        "dialogMessageResId",
        "buttonAction",
    ] {
        for value in [
            Value::Int(0),
            Value::Int(-1),
            Value::Int(0x7f010001),
            Value::Int(0x81010001u32 as i32),
            Value::Int(0x7f000001),
            Value::Int(1),
            Value::Int(2),
            Value::String("bad".into()),
            Value::Long(1),
            Value::Null,
        ] {
            let mut e = aim_android_xml::read(b"<dialog-info iconResId='2130771969' title='title' buttonText='button' dialogMessage='message' buttonAction='1'/>").unwrap();
            e.attrs.retain(|(name, _)| name != field);
            e.attrs.push((field.into(), value));
            inputs.push(e);
        }
    }
    for field in ["title", "buttonText", "dialogMessage"] {
        for value in [
            Value::String(String::new()),
            Value::Null,
            Value::Int(17),
            Value::String("α & message".into()),
        ] {
            let mut e = aim_android_xml::read(b"<dialog-info iconResId='2130771969' title='title' buttonText='button' dialogMessage='message' buttonAction='1'/>").unwrap();
            e.attrs.retain(|(name, _)| name != field);
            e.attrs.push((field.into(), value));
            inputs.push(e);
        }
    }
    let mut expected = Vec::new();
    for e in &inputs {
        let index = expected.len();
        fs::write(
            directory.join(format!("{index}.xml")),
            aim_android_xml::abx::write(e).unwrap(),
        )
        .unwrap();
        expected.push(DialogInfo::restore(e).save("dialog-info"));
    }
    // Text parser conversions are checked separately from the ABX typed values.
    for xml in [
        "<dialog-info title='text' buttonAction='1'/>",
        "<dialog-info title='' dialogMessage='lost'/>",
        "<dialog-info iconResId='bad' titleResId='-2130640895' title='ignored' buttonAction='bad'/>",
    ] {
        let index = expected.len();
        fs::write(directory.join(format!("{index}.xml")), xml).unwrap();
        expected.push(
            DialogInfo::restore(&aim_android_xml::read(xml.as_bytes()).unwrap())
                .save("dialog-info"),
        );
    }
    let count = expected.len().to_string();
    let result = run(boot.command().args([
        "shell",
        "/system/bin/app_process",
        "-Djava.class.path=/data/local/tmp/suspension-dialogs/oracle.dex",
        "/system/bin",
        "SuspensionDialogOracle",
        "/data/local/tmp/suspension-dialogs",
        &count,
    ]));
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        format!("DIALOGS {count}\n")
    );
    for (index, native) in expected.into_iter().enumerate() {
        let original: Element =
            aim_android_xml::read(&fs::read(directory.join(format!("{index}.original"))).unwrap())
                .unwrap();
        assert_eq!(native, original, "case {index}");
    }
    check_extras(&boot, &directory);
}

fn check_extras(boot: &Boot, directory: &std::path::Path) {
    let mut cases = Vec::new();
    for content in [
        "",
        "<null name='n'/><int name='i' value='-7'/><long name='l' value='9223372036854775807'/><boolean name='b' value='true'/><string name='s'>α &amp; message</string>",
        "<string name='s'>a<![CDATA[b]]><!-- skipped -->c</string>",
        "<string name='s'><![CDATA[standalone]]>tail<![CDATA[more]]></string>",
        "<string name='s'><![CDATA[standalone]]></string>",
        "<int name='duplicate' value='1'/><string name='duplicate'>last</string>",
        "<int name='empty-text' value='1'><![CDATA[]]></int>",
        "<int name='duplicate' value='1'/><float name='duplicate' value='2.5'/>",
        "<float name='f' value='2.5'/><byte-array name='bytes' num='2'>01ff</byte-array><list name='list'><int value='3'/></list><set name='set'><string>x</string></set><int name='kept' value='1'/>",
        "<pbundle_as_map name='nested'><int name='x' value='2'/><pbundle_as_map name='deep'><string name='v'>yes</string></pbundle_as_map></pbundle_as_map>",
        "<map name='nested'><float name='drop' value='2'/><boolean name='keep' value='false'/></map>",
        "<int value='3'/><int value='4'/>",
        "<string name='BB'>first collision</string><string name='Aa'>second collision</string><int name='' value='1'/><int value='2'/><boolean name='negative-hash-key' value='true'/><string name='😀'>utf16 hash</string>",
        "<unknown name='x'/>",
        "<int name='bad'/>",
        "<int name='bad' value='bad'/>",
        "<int name='bad' value='1'> </int>",
        "<string name='bad'><int value='1'/></string>",
        "<byte-array name='bytes' num='2'>01</byte-array>",
        "<byte-array name='bytes' num='1'>zz</byte-array>",
        "<byte-array name='bytes' num='1'>0<![CDATA[1]]></byte-array>",
        "<pbundle_as_map name='empty'/>",
    ] {
        cases.push(format!("<app-extras>{content}</app-extras>"));
    }
    for value in [
        "0x1.0000010000000001p0",
        "0x1.000001p0",
        "0x1.000003p0",
        "0x1p-149",
        "0x1p-150",
        "-0.0f",
        "0x1.fffffep127",
        "0x1p128",
        "1.25D",
        "NaN",
        "-NaN",
        "Infinity",
        "inf",
        "nan",
        "1_0",
    ] {
        cases.push(format!("<float-value value='{value}'/>"));
        cases.push(format!(
            "<app-extras><float name='discard' value='{value}'/></app-extras>"
        ));
    }
    for (kind, value) in [
        ("int-array", "7"),
        ("long-array", "9223372036854775807"),
        ("double-array", "-0.0"),
        ("boolean-array", "true"),
        ("string-array", "hello"),
    ] {
        for (num, items) in [(0, 0), (3, 0), (3, 1), (2, 2), (1, 2), (-1, 0)] {
            let item = format!("<item value='{value}'/>");
            cases.push(format!(
                "<app-extras><{kind} name='a' num='{num}'>{}</{kind}></app-extras>",
                item.repeat(items)
            ));
        }
        cases.push(format!(
            "<app-extras><{kind} name='a' num='1'><item/></{kind}></app-extras>"
        ));
        cases.push(format!(
            "<app-extras><{kind} name='a' num='1'><bad/></{kind}></app-extras>"
        ));
    }
    for value in [
        "0",
        "-0.0",
        "NaN",
        "-NaN",
        "Infinity",
        "-Infinity",
        "1.25D",
        "0x1p-1074",
        "0x1p-1075",
        "0x1.0000000000001p-1075",
        "0x1.00000000000008p0",
        "0x1.00000000000018p0",
        "0x1.fffffffffffffp1023",
        "0x1p999999999999999999",
        "1e9999",
        "inf",
        "nan",
        "InfinityD",
        "0x1",
        "1_0",
    ] {
        cases.push(format!(
            "<app-extras><double name='d' value='{value}'/></app-extras>"
        ));
    }
    for content in [
        "<dialog-info title='kept'/><app-extras><int name='i' value='3'/></app-extras><launcher-extras><string name='s'>ok</string></launcher-extras>",
        "<dialog-info title='kept'/><app-extras><unknown/></app-extras><dialog-info title='lost'/>",
        "<app-extras><int name='i' value='3'/></app-extras><app-extras><unknown/></app-extras>",
        "<app-extras/><launcher-extras><int name='i' value='3'/></launcher-extras>",
        "<app-extras/>",
        "<ignored><dialog-info title='nested'/><launcher-extras><boolean name='b' value='true'/></launcher-extras></ignored>",
        "<app-extras><int-array name='a' num='-1'/></app-extras>",
        "<app-extras><int-array name='a' num='0'><item value='1'/></int-array></app-extras>",
    ] {
        cases.push(format!("<suspend-params suspending-package='android' quarantined='true'>{content}</suspend-params>"));
    }
    let mut expected = Vec::new();
    let mut parcels = Vec::new();
    // Run both text and ABX parsers on every fixture.
    for case in &cases {
        let root = aim_android_xml::read(case.as_bytes()).unwrap();
        for bytes in [
            case.as_bytes().to_vec(),
            aim_android_xml::abx::write(&root).unwrap(),
        ] {
            let index = expected.len();
            fs::write(directory.join(format!("extras-{index}.xml")), &bytes).unwrap();
            let root = aim_android_xml::read_next(&bytes).unwrap();
            expected.push(native_extras(&root));
            if root.name == "app-extras" {
                if let Ok(value) =
                    aim_services::package::restrictions::persistable::Bundle::restore(&root)
                {
                    parcels.push(value);
                }
            }
        }
    }
    // Typed ABX conversions differ from text even when getAttributeValue looks alike.
    for value in [
        Value::Double(-0.0),
        Value::Double(f64::NAN),
        Value::Double(f64::from_bits(0xfff0000000000001)),
        Value::Double(f64::INFINITY),
        Value::Int(1),
        Value::Long(1),
        Value::Float(1.0),
        Value::Null,
    ] {
        let mut root =
            aim_android_xml::read(b"<app-extras><double name='d' value='0'/></app-extras>")
                .unwrap();
        let aim_android_xml::Node::Element(child) = &mut root.content[0] else {
            unreachable!()
        };
        child.attrs.retain(|(name, _)| name != "value");
        child.attrs.push(("value".into(), value));
        let index = expected.len();
        fs::write(
            directory.join(format!("extras-{index}.xml")),
            aim_android_xml::abx::write(&root).unwrap(),
        )
        .unwrap();
        expected.push(native_extras(&root));
        if let Ok(value) = aim_services::package::restrictions::persistable::Bundle::restore(&root)
        {
            parcels.push(value);
        }
    }
    let count = expected.len().to_string();
    let mut parcel_expected = Vec::new();
    for (index, value) in parcels.iter().enumerate() {
        let mut parcel = value.parcel().unwrap();
        parcel.write_i32(0x11ddee55);
        fs::write(
            directory.join(format!("parcel-{index}.native")),
            parcel.data(),
        )
        .unwrap();
        let mut semantic = Vec::new();
        bundle(&mut semantic, Some(value));
        parcel_expected.push((parcel.data().to_vec(), semantic));
    }
    let parcel_count = parcels.len().to_string();
    let output = run(boot.command().args(["shell", "/system/bin/app_process", "-Djava.class.path=/data/local/tmp/suspension-dialogs/oracle.dex:/system/framework/services.jar", "/system/bin", "PersistableBundleOracle", "/data/local/tmp/suspension-dialogs", &count, &parcel_count]));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("EXTRAS {count}\nBUNDLES {parcel_count}\n")
    );
    for (index, bytes) in expected.into_iter().enumerate() {
        assert_eq!(
            bytes,
            fs::read(directory.join(format!("extras-{index}.original"))).unwrap(),
            "extras case {index}: {}",
            cases
                .get(index / 2)
                .map(String::as_str)
                .unwrap_or("typed ABX")
        );
    }
    eprintln!("verified {count} original extras/parameter cases");
    for (index, (bytes, semantic)) in parcel_expected.into_iter().enumerate() {
        assert_eq!(
            semantic,
            fs::read(directory.join(format!("parcel-{index}.semantic"))).unwrap(),
            "native bundle values {index}"
        );
        assert_eq!(
            bytes,
            fs::read(directory.join(format!("parcel-{index}.original"))).unwrap(),
            "native bundle bytes {index}"
        );
    }
    eprintln!("verified {parcel_count} native bundles through original Parcel read/write");
}

fn text(out: &mut Vec<u8>, value: Option<&str>) {
    out.extend_from_slice(&value.map_or(-1, |s| s.len() as i32).to_be_bytes());
    if let Some(value) = value {
        out.extend_from_slice(value.as_bytes());
    }
}

fn bundle(
    out: &mut Vec<u8>,
    value: Option<&aim_services::package::restrictions::persistable::Bundle>,
) {
    use aim_services::package::restrictions::persistable::Value;
    let Some(value) = value else {
        out.extend_from_slice(&(-1i32).to_be_bytes());
        return;
    };
    let mut entries = value.entries.iter().collect::<Vec<_>>();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    out.extend_from_slice(&(entries.len() as i32).to_be_bytes());
    for (key, value) in entries {
        text(out, key.as_deref());
        match value {
            Value::Null => out.push(0),
            Value::Int(v) => {
                out.push(1);
                out.extend_from_slice(&v.to_be_bytes());
            }
            Value::Long(v) => {
                out.push(2);
                out.extend_from_slice(&v.to_be_bytes());
            }
            Value::Double(v) => {
                out.push(3);
                out.extend_from_slice(&v.bits().to_be_bytes());
            }
            Value::Bool(v) => {
                out.push(4);
                out.push(u8::from(*v));
            }
            Value::String(v) => {
                out.push(5);
                text(out, Some(v));
            }
            Value::Ints(v) => {
                out.push(6);
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                for x in v {
                    out.extend_from_slice(&x.to_be_bytes());
                }
            }
            Value::Longs(v) => {
                out.push(7);
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                for x in v {
                    out.extend_from_slice(&x.to_be_bytes());
                }
            }
            Value::Doubles(v) => {
                out.push(8);
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                for x in v {
                    out.extend_from_slice(&x.bits().to_be_bytes());
                }
            }
            Value::Bools(v) => {
                out.push(9);
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                for x in v {
                    out.push(u8::from(*x));
                }
            }
            Value::Strings(v) => {
                out.push(10);
                out.extend_from_slice(&(v.len() as i32).to_be_bytes());
                for x in v {
                    text(out, x.as_deref());
                }
            }
            Value::Bundle(v) => {
                out.push(11);
                bundle(out, Some(v));
            }
        }
    }
}

fn native_extras(root: &Element) -> Vec<u8> {
    use aim_services::package::restrictions::{
        Restrictions,
        persistable::{Bundle, Error},
    };
    let mut out = vec![0];
    if root.name == "float-value" {
        let Ok(Some(value)) = root.float("value") else {
            return vec![1];
        };
        out.extend_from_slice(
            &if value.is_nan() {
                0x7fc00000u32
            } else {
                value.to_bits()
            }
            .to_be_bytes(),
        );
    } else if root.name == "suspend-params" {
        let wrapper = Element {
            name: "package-restrictions".into(),
            attrs: vec![],
            content: vec![aim_android_xml::Node::Element(Element {
                name: "pkg".into(),
                attrs: vec![("name".into(), Value::String("test".into()))],
                content: vec![aim_android_xml::Node::Element(root.clone())],
            })],
        };
        let Ok(state) = Restrictions::parse(&wrapper) else {
            return vec![2];
        };
        let s = &state.packages[0].1.suspensions[0];
        out.push(u8::from(s.quarantined));
        out.push(u8::from(s.dialog.is_some()));
        if let Some(d) = &s.dialog {
            out.extend_from_slice(&d.icon.to_be_bytes());
            out.extend_from_slice(&d.title_resource.to_be_bytes());
            text(&mut out, d.title.as_deref());
            out.extend_from_slice(&d.message_resource.to_be_bytes());
            text(&mut out, d.message.as_deref());
            out.extend_from_slice(&d.button_resource.to_be_bytes());
            text(&mut out, d.button.as_deref());
            out.extend_from_slice(&d.button_action.to_be_bytes());
        }
        bundle(&mut out, s.app_extras.as_ref());
        bundle(&mut out, s.launcher_extras.as_ref());
    } else {
        match Bundle::restore(root) {
            Ok(value) => bundle(&mut out, Some(&value)),
            Err(Error::Xml(_)) => return vec![1],
            Err(Error::Runtime(_)) => return vec![2],
        }
    }
    out
}
