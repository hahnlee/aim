//! The volume groups of `IConfig`'s engine configuration: one per stream,
//! with the curves of the image's volume tables, as the reference HAL's
//! `AudioPolicyConfigXmlConverter` builds them from the volumes that
//! `audio_policy_configuration.xml` includes. Without them every group of
//! the policy engine has the index range 0 to 0 and no curve, so AudioService
//! has no volume steps at all.
//!
//! The tables are Android's legacy volume XML: `<reference name>` curves in
//! `default_volume_tables.xml`, and a `<volume stream deviceCategory>` of
//! `<point>index,millibels</point>` or a `ref` per stream and device
//! category in `audio_policy_volumes.xml`.

use std::collections::HashMap;

use android_media_audio_common_types::aidl::android::media::audio::common::{
    AudioHalVolumeCurve::{
        AudioHalVolumeCurve,
        CurvePoint::{self as point, CurvePoint},
        DeviceCategory::DeviceCategory,
    },
    AudioHalVolumeGroup::{AudioHalVolumeGroup, INDEX_DEFERRED_TO_AUDIO_SERVICE},
};

/// The image's volume tables, references first.
pub const FILES: [&str; 2] = [
    "/vendor/etc/default_volume_tables.xml",
    "/vendor/etc/audio_policy_volumes.xml",
];

/// Streams that are not public: the engine's own range, as the reference
/// HAL gives them (`kDefaultVolumeIndexMin`, `kDefaultVolumeIndexMax`).
const INTERNAL_STREAMS: [&str; 3] = [
    "AUDIO_STREAM_REROUTING",
    "AUDIO_STREAM_PATCH",
    "AUDIO_STREAM_CALL_ASSISTANT",
];

/// One element of the XML: its name, attributes, and whether it closes
/// (`</x>`) or is empty (`<x/>`).
#[derive(Debug, PartialEq)]
enum Token<'a> {
    Open(&'a str, Vec<(&'a str, &'a str)>, bool),
    Close(&'a str),
    Text(&'a str),
}

/// The elements and text of `xml`, without the prolog and comments.
fn tokens(xml: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut rest = xml;
    while !rest.is_empty() {
        let Some(start) = rest.find('<') else {
            out.push(Token::Text(rest));
            break;
        };
        if start > 0 {
            out.push(Token::Text(&rest[..start]));
        }
        rest = &rest[start..];
        let end = if rest.starts_with("<!--") {
            rest.find("-->").map(|e| e + 3)
        } else {
            rest.find('>').map(|e| e + 1)
        };
        let Some(end) = end else { break };
        let tag = &rest[1..end - 1];
        rest = &rest[end..];
        if tag.starts_with('!') || tag.starts_with('?') {
            continue;
        }
        if let Some(name) = tag.strip_prefix('/') {
            out.push(Token::Close(name.trim()));
            continue;
        }
        let empty = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let (name, mut attrs) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let mut attributes = Vec::new();
        while let Some((key, value)) = attrs.split_once('=') {
            let value = value.trim_start();
            let Some(quote) = value.chars().next().filter(|q| *q == '"' || *q == '\'') else {
                break;
            };
            let Some(close) = value[1..].find(quote) else {
                break;
            };
            attributes.push((key.trim(), &value[1..1 + close]));
            attrs = &value[close + 2..];
        }
        out.push(Token::Open(name, attributes, empty));
    }
    out
}

fn attribute<'a>(attributes: &[(&str, &'a str)], key: &str) -> Option<&'a str> {
    attributes.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}

/// `index,millibels`.
fn point(text: &str) -> Option<CurvePoint> {
    let (index, attenuation) = text.trim().split_once(',')?;
    Some(CurvePoint {
        index: index.trim().parse().ok()?,
        attenuationMb: attenuation.trim().parse().ok()?,
    })
}

fn category(name: &str) -> Option<DeviceCategory> {
    Some(match name {
        "DEVICE_CATEGORY_HEADSET" => DeviceCategory::HEADSET,
        "DEVICE_CATEGORY_SPEAKER" => DeviceCategory::SPEAKER,
        "DEVICE_CATEGORY_EARPIECE" => DeviceCategory::EARPIECE,
        "DEVICE_CATEGORY_EXT_MEDIA" => DeviceCategory::EXT_MEDIA,
        "DEVICE_CATEGORY_HEARING_AID" => DeviceCategory::HEARING_AID,
        _ => return None,
    })
}

/// The volume groups of the tables in `documents`, in the order their
/// streams first appear. A curve with an unknown device category or
/// reference is left out, with a warning.
pub fn groups(documents: &[String]) -> Vec<AudioHalVolumeGroup> {
    let mut references: HashMap<String, Vec<(i8, i32)>> = HashMap::new();
    let mut groups: Vec<AudioHalVolumeGroup> = Vec::new();
    for document in documents {
        // The element being read: a reference's name, or a volume's stream
        // and category; and its points.
        let mut reference: Option<&str> = None;
        let mut volume: Option<(&str, &str)> = None;
        let mut points: Vec<(i8, i32)> = Vec::new();
        let mut in_point = false;
        for token in tokens(document) {
            match token {
                Token::Open("reference", attributes, false) => {
                    reference = attribute(&attributes, "name");
                    points.clear();
                }
                Token::Open("volume", attributes, empty) => {
                    let (Some(stream), Some(category)) = (
                        attribute(&attributes, "stream"),
                        attribute(&attributes, "deviceCategory"),
                    ) else {
                        continue;
                    };
                    points.clear();
                    match (empty, attribute(&attributes, "ref")) {
                        (true, Some(name)) => match references.get(name) {
                            Some(curve) => add(&mut groups, stream, category, curve),
                            None => log::warn!("volume {stream} {category}: no curve {name}"),
                        },
                        (false, _) => volume = Some((stream, category)),
                        (true, None) => {}
                    }
                }
                Token::Open("point", _, false) => in_point = true,
                Token::Text(text) if in_point => {
                    if let Some(p) = point(text) {
                        points.push((p.index, p.attenuationMb));
                    }
                }
                Token::Close("point") => in_point = false,
                Token::Close("reference") => {
                    if let Some(name) = reference.take() {
                        references.insert(name.to_string(), std::mem::take(&mut points));
                    }
                }
                Token::Close("volume") => {
                    if let Some((stream, category)) = volume.take() {
                        add(&mut groups, stream, category, &points);
                    }
                }
                _ => {}
            }
        }
    }
    groups
}

fn add(groups: &mut Vec<AudioHalVolumeGroup>, stream: &str, name: &str, points: &[(i8, i32)]) {
    let Some(category) = category(name) else {
        log::warn!("volume {stream}: unknown device category {name}");
        return;
    };
    let index = match groups.iter().position(|g| g.name == stream) {
        Some(i) => i,
        None => {
            let range = if INTERNAL_STREAMS.contains(&stream) {
                (point::MIN_INDEX as i32, point::MAX_INDEX as i32)
            } else {
                (
                    INDEX_DEFERRED_TO_AUDIO_SERVICE,
                    INDEX_DEFERRED_TO_AUDIO_SERVICE,
                )
            };
            groups.push(AudioHalVolumeGroup {
                name: stream.to_string(),
                minIndex: range.0,
                maxIndex: range.1,
                volumeCurves: Vec::new(),
            });
            groups.len() - 1
        }
    };
    groups[index].volumeCurves.push(AudioHalVolumeCurve {
        deviceCategory: category,
        curvePoints: points
            .iter()
            .map(|&(index, attenuation)| CurvePoint {
                index,
                attenuationMb: attenuation,
            })
            .collect(),
    });
}
