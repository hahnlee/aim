//! Narrow recovery for a DATA mount missing from the IORegistry inventory.

use super::*;
use aim_android_xml::{Element, Node, TEXT};

pub(crate) fn leftover_devices(image: &Path, mount: &Path) -> Result<Vec<String>, String> {
    let output = run(Command::new("hdiutil").args(["info", "-plist"]))?;
    devices(&output.stdout, image, mount).map_err(|error| format!("hdiutil info: {error}"))
}

fn field<'a>(dict: &'a Element, key: &str) -> Result<Option<&'a Element>, String> {
    if dict.name != "dict" {
        return Err("expected a dictionary".into());
    }
    let children: Vec<_> = dict.children().collect();
    if children.len() % 2 != 0 {
        return Err("unpaired dictionary key".into());
    }
    let mut found = None;
    for pair in children.chunks_exact(2) {
        if pair[0].name != "key" {
            return Err("expected a dictionary key".into());
        }
        if text(pair[0])? == key && found.replace(pair[1]).is_some() {
            return Err(format!("duplicate {key}"));
        }
    }
    Ok(found)
}

fn text(element: &Element) -> Result<String, String> {
    let mut value = String::new();
    for node in &element.content {
        match node {
            Node::Token(TEXT | aim_android_xml::CDSECT, Some(part)) => value.push_str(part),
            Node::Token(aim_android_xml::COMMENT, _) => (),
            _ => return Err("expected text".into()),
        }
    }
    Ok(value)
}

fn string(dict: &Element, key: &str) -> Result<Option<String>, String> {
    field(dict, key)?
        .map(|value| {
            if value.name != "string" {
                return Err(format!("{key}: expected a string"));
            }
            text(value)
        })
        .transpose()
}

fn array<'a>(dict: &'a Element, key: &str) -> Result<&'a Element, String> {
    field(dict, key)?
        .filter(|value| value.name == "array")
        .ok_or_else(|| format!("{key}: expected an array"))
}

fn devices(bytes: &[u8], image: &Path, mount: &Path) -> Result<Vec<String>, String> {
    let plist = aim_android_xml::read(bytes)?;
    if plist.name != "plist" || plist.children().count() != 1 {
        return Err("expected a plist dictionary".into());
    }
    let mut found = Vec::new();
    for attached in array(plist.children().next().unwrap(), "images")?.children() {
        if string(attached, "image-path")?.as_deref().map(Path::new) != Some(image) {
            continue;
        }
        if field(attached, "shadow-path")?.is_some() {
            return Err("DATA image has a shadow".into());
        }
        let mut device = None;
        let mut mounted = false;
        for entity in array(attached, "system-entities")?.children() {
            if let Some(path) = string(entity, "mount-point")? {
                if Path::new(&path) != mount {
                    return Err("DATA image has a foreign mount".into());
                }
                mounted = true;
            }
            if device.is_none() {
                device = string(entity, "dev-entry")?.filter(|path| {
                    path.strip_prefix("/dev/disk")
                        .is_some_and(|n| !n.is_empty() && n.bytes().all(|c| c.is_ascii_digit()))
                });
            }
        }
        if mounted {
            let device = device.ok_or("mounted DATA image has no whole disk device")?;
            if !found.contains(&device) {
                found.push(device);
            }
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    // hdiutil info -plist's image/device structure, with portable paths.
    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>images</key><array>
<dict><key>image-path</key><string>@IMAGE@</string>
<key>system-entities</key><array>
<dict><key>dev-entry</key><string>/dev/disk91</string></dict>
<dict><key>dev-entry</key><string>/dev/disk92s1</string>
<key>mount-point</key><string>@MOUNT@</string></dict>
</array></dict>
<dict><key>image-path</key><string>@IMAGE@.other</string>
<key>system-entities</key><array><dict>
<key>dev-entry</key><string>/dev/disk80</string>
<key>mount-point</key><string>/Volumes/User Data</string>
</dict></array></dict></array></dict></plist>"#;

    fn paths() -> (PathBuf, PathBuf) {
        let mount = aim_paths::out().join("userdata-template.work/boot");
        (crate::data::image_of(&mount), mount)
    }

    fn fixture(image: &Path, mount: &Path) -> String {
        let escape = |path: &Path| path.to_str().unwrap().replace('&', "&amp;");
        FIXTURE
            .replace("@IMAGE@", &escape(image))
            .replace("@MOUNT@", &escape(mount))
    }

    #[test]
    fn only_exact_work_image_and_mount_match() {
        let (image, mount) = paths();
        assert_eq!(
            devices(fixture(&image, &mount).as_bytes(), &image, &mount).unwrap(),
            ["/dev/disk91"]
        );
        for foreign in [
            aim_paths::root()
                .with_file_name("other-worktree")
                .join("target/aim/userdata-template.work/boot"),
            mount.with_file_name("boot-copy"),
            mount.join("nested"),
            PathBuf::from("/Volumes/User Data"),
        ] {
            // Even a foreign image at our mount is never ours to detach.
            assert!(
                devices(
                    fixture(&crate::data::image_of(&foreign), &mount).as_bytes(),
                    &image,
                    &mount
                )
                .unwrap()
                .is_empty()
            );
            assert!(devices(fixture(&image, &foreign).as_bytes(), &image, &mount).is_err());
        }
    }

    #[test]
    fn escaped_paths_and_empty_inventory() {
        let (image, mount) = paths();
        let mount = mount.with_file_name("boot & data");
        let image = image.with_file_name("boot & data.asif");
        assert_eq!(
            devices(fixture(&image, &mount).as_bytes(), &image, &mount).unwrap(),
            ["/dev/disk91"]
        );
        assert!(
            devices(
                b"<plist><dict><key>images</key><array/></dict></plist>",
                &image,
                &mount
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn malformed_shadow_and_missing_device_are_refused() {
        let (image, mount) = paths();
        let valid = fixture(&image, &mount);
        for invalid in [
            "not a plist".into(),
            valid.replace(
                "<key>system-entities</key>",
                "<key>shadow-path</key><string>foreign.shadow</string><key>system-entities</key>",
            ),
            valid.replace("/dev/disk91", "/dev/disk91s1"),
            valid.replace(
                "<key>image-path</key>",
                "<key>image-path</key><string>duplicate</string><key>image-path</key>",
            ),
        ] {
            assert!(
                devices(invalid.as_bytes(), &image, &mount).is_err(),
                "{invalid}"
            );
        }
    }
}
