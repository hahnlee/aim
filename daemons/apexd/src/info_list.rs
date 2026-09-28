//! `/apex/apex-info-list.xml` (apexd's `ApexInfoList.xsd`) as `ApexInfo`s.
//! The file is written by guest-init: one `<apex-info .../>` element per
//! APEX, with attributes only.

use android_apex::aidl::android::apex::ApexInfo::{ApexInfo, Partition::Partition};

/// The value of `name="..."` in an element's attribute text.
fn attr<'a>(element: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let at = element.find(&key)? + key.len();
    let end = element[at..].find('"')?;
    Some(&element[at..at + end])
}

fn unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn partition(name: &str) -> Partition {
    match name {
        "SYSTEM_EXT" => Partition::SYSTEM_EXT,
        "PRODUCT" => Partition::PRODUCT,
        "VENDOR" => Partition::VENDOR,
        "ODM" => Partition::ODM,
        _ => Partition::SYSTEM,
    }
}

pub fn parse(text: &str) -> Vec<ApexInfo> {
    text.split("<apex-info")
        .skip(1)
        .filter_map(|rest| {
            let element = &rest[..rest.find('>')?];
            let s = |name| attr(element, name).map(unescape).unwrap_or_default();
            let flag = |name| attr(element, name) == Some("true");
            Some(ApexInfo {
                moduleName: s("moduleName"),
                modulePath: s("modulePath"),
                preinstalledModulePath: s("preinstalledModulePath"),
                versionCode: attr(element, "versionCode")?.parse().ok()?,
                versionName: s("versionName"),
                isFactory: flag("isFactory"),
                isActive: flag("isActive"),
                partition: partition(attr(element, "partition").unwrap_or("SYSTEM")),
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_guest_init_s_list() {
        let list = parse(
            r#"<?xml version="1.0" encoding="utf-8"?>
<apex-info-list>
    <apex-info moduleName="com.android.art" modulePath="/system/apex/com.google.android.art.capex" preinstalledModulePath="/system/apex/com.google.android.art.capex" versionCode="360499999" versionName="" isFactory="true" isActive="true" provideSharedApexLibs="false" partition="SYSTEM"/>
    <apex-info moduleName="com.android.hardware.x" modulePath="/vendor/apex/x.apex" preinstalledModulePath="/vendor/apex/x.apex" versionCode="1" versionName="a&amp;b" isFactory="true" isActive="false" partition="VENDOR"/>
</apex-info-list>"#,
        );
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].moduleName, "com.android.art");
        assert_eq!(list[0].versionCode, 360499999);
        assert!(list[0].isActive && list[0].isFactory);
        assert_eq!(list[1].partition, Partition::VENDOR);
        assert_eq!(list[1].versionName, "a&b");
        assert!(!list[1].isActive);
    }
}
