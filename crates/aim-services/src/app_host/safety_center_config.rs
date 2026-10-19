//! The Safety Center configuration (`android.safetycenter.config` at
//! `android-16.0.0_r1`): `SafetyCenterConfigParser.parseXmlResource` of
//! the resources APK's `raw/safety_center_config`, the builders'
//! validation (`SafetyCenterConfig`, `SafetySourcesGroup`, `SafetySource`,
//! `BuilderUtils`) and the parcels `getSafetyCenterConfig` answers with.
//! The parser's `nextTag` is followed over the document's tree: whitespace
//! and comments between tags are skipped, other text is an error.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use aim_android_xml::{Element, Node, TEXT, Value};
use aim_binder_host::parcel::Parcel;
use aim_service_aidl::WriteParcelable;

/// `SafetySource.SAFETY_SOURCE_TYPE_*`.
pub const TYPE_STATIC: i32 = 1;
pub const TYPE_DYNAMIC: i32 = 2;
pub const TYPE_ISSUE_ONLY: i32 = 3;
/// `SafetySource.PROFILE_*`.
const PROFILE_NONE: i32 = 0;
const PROFILE_PRIMARY: i32 = 1;
const PROFILE_ALL: i32 = 2;
/// `SafetySource.INITIAL_DISPLAY_STATE_*`.
const DISPLAY_ENABLED: i32 = 0;
const DISPLAY_DISABLED: i32 = 1;
const DISPLAY_HIDDEN: i32 = 2;
/// `SafetySourcesGroup.SAFETY_SOURCES_GROUP_TYPE_*`.
const GROUP_STATEFUL: i32 = 0;
const GROUP_STATELESS: i32 = 1;
const GROUP_HIDDEN: i32 = 2;
/// `SafetySourcesGroup.STATELESS_ICON_TYPE_*`.
const ICON_NONE: i32 = 0;
const ICON_PRIVACY: i32 = 1;
/// `Resources.ID_NULL`.
const ID_NULL: i32 = 0;

const TAG_CONFIG: &str = "safety-center-config";
const TAG_SOURCES_CONFIG: &str = "safety-sources-config";
const TAG_GROUP: &str = "safety-sources-group";

/// What the parser asks of the resources APK's `Resources`.
pub trait StringResources {
    /// `getIdentifier(entry, type, package)`, `ID_NULL` as none.
    fn identifier(&self, package: &str, kind: &str, entry: &str) -> Option<u32>;
    /// `getString(id)`.
    fn string(&self, id: u32) -> Result<String, String>;
}

/// The flags the parser and the parcels follow.
#[derive(Clone, Copy)]
pub struct Flags {
    /// `com.android.permission.flags.private_profile_title_api`.
    pub private_profile_title_api: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SafetySource {
    pub kind: i32,
    pub id: String,
    pub package_name: Option<String>,
    pub title: i32,
    pub title_for_work: i32,
    pub summary: i32,
    pub intent_action: Option<String>,
    pub profile: i32,
    pub initial_display_state: i32,
    pub max_severity_level: i32,
    pub search_terms: i32,
    pub logging_allowed: bool,
    pub refresh_on_page_open_allowed: bool,
    pub notifications_allowed: bool,
    pub deduplication_group: Option<String>,
    /// `Set.copyOf` of the hashes, kept in the order they were added.
    pub package_certificate_hashes: Vec<String>,
    pub title_for_private_profile: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SafetySourcesGroup {
    pub kind: i32,
    pub id: String,
    pub title: i32,
    pub summary: i32,
    pub stateless_icon_type: i32,
    pub sources: Vec<SafetySource>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SafetyCenterConfig {
    pub groups: Vec<SafetySourcesGroup>,
}

/// A `SafetyCenterConfig` written for `flags`.
pub struct Written<'a>(pub &'a SafetyCenterConfig, pub Flags);

impl WriteParcelable for Written<'_> {
    fn write_to(&self, p: &mut Parcel) {
        // `writeTypedList`.
        p.write_i32(self.0.groups.len() as i32);
        for g in &self.0.groups {
            p.write_i32(1);
            p.write_string16(Some(&g.id));
            p.write_i32(g.title);
            p.write_i32(g.summary);
            p.write_i32(g.stateless_icon_type);
            p.write_i32(g.sources.len() as i32);
            for s in &g.sources {
                p.write_i32(1);
                p.write_i32(s.kind);
                p.write_string16(Some(&s.id));
                p.write_string16(s.package_name.as_deref());
                p.write_i32(s.title);
                p.write_i32(s.title_for_work);
                p.write_i32(s.summary);
                p.write_string16(s.intent_action.as_deref());
                p.write_i32(s.profile);
                p.write_i32(s.initial_display_state);
                p.write_i32(s.max_severity_level);
                p.write_i32(s.search_terms);
                p.write_bool(s.logging_allowed);
                p.write_bool(s.refresh_on_page_open_allowed);
                p.write_bool(s.notifications_allowed);
                p.write_string16(s.deduplication_group.as_deref());
                let hashes: Vec<Option<String>> = s
                    .package_certificate_hashes
                    .iter()
                    .cloned()
                    .map(Some)
                    .collect();
                aim_service_aidl::write_string_list(p, Some(&hashes));
                if self.1.private_profile_title_api {
                    p.write_i32(s.title_for_private_profile);
                }
            }
            p.write_i32(g.kind);
        }
    }
}

/// A parse failure: its message (`ParseException`).
type Parse<T> = Result<T, String>;

/// `parseXmlResource`.
pub fn parse(
    xml: &[u8],
    resources: &dyn StringResources,
    flags: Flags,
) -> Parse<SafetyCenterConfig> {
    let root = aim_android_xml::read_next(xml)
        .map_err(|e| format!("Exception while parsing the XML resource: {e}"))?;
    if local(&root.name) != TAG_CONFIG {
        return Err(element_missing(TAG_CONFIG));
    }
    let p = Parser { resources, flags };
    p.config(&root)
}

struct Parser<'a> {
    resources: &'a dyn StringResources,
    flags: Flags,
}

/// The name a namespace-processing parser reports.
fn local(name: &str) -> &str {
    name.rsplit_once(':').map_or(name, |(_, n)| n)
}

/// The attributes a namespace-processing parser reports, in order.
fn attributes(e: &Element) -> Vec<(&str, String)> {
    e.attrs
        .iter()
        .filter(|(n, _)| n != "xmlns" && !n.starts_with("xmlns:"))
        .map(|(n, v)| (local(n), value_text(v)))
        .collect()
}

fn value_text(v: &Value) -> String {
    v.string().map(|s| s.into_owned()).unwrap_or_default()
}

/// The child elements `nextTag` meets, in order: whitespace text and
/// other events skipped, non-whitespace text an error.
fn tags(e: &Element) -> Parse<Vec<&Element>> {
    let mut out = Vec::new();
    for node in &e.content {
        match node {
            Node::Element(child) => out.push(child),
            Node::Token(TEXT, Some(t))
                if !t.trim_matches(|c: char| c.is_ascii_whitespace()).is_empty() =>
            {
                return Err("Exception while parsing the XML resource: unexpected text".into());
            }
            Node::Token(..) => {}
        }
    }
    Ok(out)
}

fn element_missing(name: &str) -> String {
    format!("Element {name} missing")
}

fn element_not_closed(name: &str) -> String {
    format!("Element {name} not closed")
}

fn element_invalid(name: &str, cause: Option<&str>) -> String {
    match cause {
        Some(cause) => format!("Element {name} invalid: {cause}"),
        None => format!("Element {name} invalid"),
    }
}

fn attribute_unexpected(parent: &str, name: &str) -> String {
    format!("Unexpected attribute {parent}.{name}")
}

fn attribute_invalid(value: &str, parent: &str, name: &str) -> String {
    format!("Attribute value \"{value}\" in {parent}.{name} invalid")
}

impl Parser<'_> {
    fn config(&self, root: &Element) -> Parse<SafetyCenterConfig> {
        if !attributes(root).is_empty() {
            return Err(element_invalid(TAG_CONFIG, None));
        }
        let children = tags(root)?;
        let Some(sources_config) = children
            .first()
            .filter(|c| local(&c.name) == TAG_SOURCES_CONFIG)
        else {
            return Err(element_missing(TAG_SOURCES_CONFIG));
        };
        if !attributes(sources_config).is_empty() {
            return Err(element_invalid(TAG_SOURCES_CONFIG, None));
        }
        let mut groups = Vec::new();
        let inner = tags(sources_config)?;
        let mut rest = inner.iter();
        let mut next = rest.next();
        while let Some(group) = next.filter(|g| local(&g.name) == TAG_GROUP) {
            groups.push(self.group(group)?);
            next = rest.next();
        }
        if next.is_some() {
            return Err(element_not_closed(TAG_SOURCES_CONFIG));
        }
        if children.len() > 1 {
            return Err(element_not_closed(TAG_CONFIG));
        }
        build_config(groups).map_err(|e| element_invalid(TAG_SOURCES_CONFIG, Some(&e)))
    }

    fn group(&self, e: &Element) -> Parse<SafetySourcesGroup> {
        let name = TAG_GROUP;
        let mut b = GroupBuilder::default();
        for (attr, value) in attributes(e) {
            match attr {
                "id" => b.id = Some(self.value(&value, name, attr)?),
                "title" => b.title = Some(self.resource_name(&value, name, attr)?),
                "summary" => b.summary = Some(self.resource_name(&value, name, attr)?),
                "statelessIconType" => {
                    b.stateless_icon_type = Some(match self.value(&value, name, attr)?.as_str() {
                        "none" => ICON_NONE,
                        "privacy" => ICON_PRIVACY,
                        v => return Err(attribute_invalid(v, name, attr)),
                    })
                }
                "type" => {
                    b.kind = Some(match self.value(&value, name, attr)?.as_str() {
                        "stateful" => GROUP_STATEFUL,
                        "stateless" => GROUP_STATELESS,
                        "hidden" => GROUP_HIDDEN,
                        v => return Err(attribute_invalid(v, name, attr)),
                    })
                }
                _ => return Err(attribute_unexpected(name, attr)),
            }
        }
        let children = tags(e)?;
        let mut rest = children.iter();
        let mut next = rest.next();
        while let Some(child) = next {
            let tag = local(&child.name);
            let kind = match tag {
                "static-safety-source" => TYPE_STATIC,
                "dynamic-safety-source" => TYPE_DYNAMIC,
                "issue-only-safety-source" => TYPE_ISSUE_ONLY,
                _ => break,
            };
            b.sources.push(self.source(child, kind, tag)?);
            next = rest.next();
        }
        if next.is_some() {
            return Err(element_not_closed(name));
        }
        b.build().map_err(|e| element_invalid(name, Some(&e)))
    }

    fn source(&self, e: &Element, kind: i32, name: &str) -> Parse<SafetySource> {
        let mut b = SourceBuilder {
            kind,
            ..Default::default()
        };
        for (attr, value) in attributes(e) {
            match attr {
                "id" => b.id = Some(self.value(&value, name, attr)?),
                "packageName" => b.package_name = Some(self.value(&value, name, attr)?),
                "title" => b.title = Some(self.resource_name(&value, name, attr)?),
                "titleForWork" => b.title_for_work = Some(self.resource_name(&value, name, attr)?),
                "titleForPrivateProfile" => {
                    // Ignored without the flag (logged by the original).
                    if self.flags.private_profile_title_api {
                        b.title_for_private_profile = Some(self.resource_name(&value, name, attr)?);
                    }
                }
                "summary" => b.summary = Some(self.resource_name(&value, name, attr)?),
                "intentAction" => b.intent_action = Some(self.value(&value, name, attr)?),
                "profile" => {
                    b.profile = Some(match self.value(&value, name, attr)?.as_str() {
                        "primary_profile_only" => PROFILE_PRIMARY,
                        "all_profiles" => PROFILE_ALL,
                        v => return Err(attribute_invalid(v, name, attr)),
                    })
                }
                "initialDisplayState" => {
                    b.initial_display_state = Some(match self.value(&value, name, attr)?.as_str() {
                        "enabled" => DISPLAY_ENABLED,
                        "disabled" => DISPLAY_DISABLED,
                        "hidden" => DISPLAY_HIDDEN,
                        v => return Err(attribute_invalid(v, name, attr)),
                    })
                }
                "maxSeverityLevel" => {
                    let v = self.value(&value, name, attr)?;
                    b.max_severity_level =
                        Some(parse_int(&v).ok_or_else(|| attribute_invalid(&v, name, attr))?);
                }
                "searchTerms" => b.search_terms = Some(self.resource_name(&value, name, attr)?),
                "loggingAllowed" => b.logging_allowed = Some(self.boolean(&value, name, attr)?),
                "refreshOnPageOpenAllowed" => {
                    b.refresh_on_page_open_allowed = Some(self.boolean(&value, name, attr)?)
                }
                "notificationsAllowed" => {
                    b.notifications_allowed = Some(self.boolean(&value, name, attr)?)
                }
                "deduplicationGroup" => {
                    b.deduplication_group = Some(self.value(&value, name, attr)?)
                }
                "packageCertificateHashes" => {
                    for hash in java_split(&self.value(&value, name, attr)?) {
                        if !b.package_certificate_hashes.iter().any(|h| h == hash) {
                            b.package_certificate_hashes.push(hash.to_owned());
                        }
                    }
                }
                _ => return Err(attribute_unexpected(name, attr)),
            }
        }
        if !tags(e)?.is_empty() {
            return Err(element_not_closed(name));
        }
        b.build(self.flags)
            .map_err(|e| element_invalid(name, Some(&e)))
    }

    /// `parseStringResourceName`: `@package:string/entry`'s id.
    fn resource_name(&self, value: &str, parent: &str, name: &str) -> Parse<i32> {
        let Some(rest) = value.strip_prefix('@') else {
            return Err(if value.is_empty() {
                format!("Resource name in {parent}.{name} cannot be empty")
            } else {
                format!("Resource name \"{value}\" in {parent}.{name} does not start with @")
            });
        };
        let Some((package, rest)) = rest.split_once(':').filter(|(p, _)| !p.is_empty()) else {
            return Err(format!(
                "Resource name \"{value}\" in {parent}.{name} does not specify a package"
            ));
        };
        let Some((kind, entry)) = rest.split_once('/').filter(|(k, _)| !k.is_empty()) else {
            return Err(format!(
                "Resource name \"{value}\" in {parent}.{name} does not specify a type"
            ));
        };
        if kind != "string" {
            return Err(format!(
                "Resource name \"{value}\" in {parent}.{name} is not a string"
            ));
        }
        match self.resources.identifier(package, kind, entry) {
            Some(id) if id != 0 => Ok(id as i32),
            _ => Err(format!(
                "Resource name \"{value}\" in {parent}.{name} missing or invalid"
            )),
        }
    }

    /// `getValueToParse`: a string resource's string, else the value.
    fn value(&self, value: &str, parent: &str, name: &str) -> Parse<String> {
        match self.resource_name(value, parent, name) {
            Ok(id) => self.resources.string(id as u32),
            Err(_) => Ok(value.to_owned()),
        }
    }

    /// `parseBoolean`.
    fn boolean(&self, value: &str, parent: &str, name: &str) -> Parse<bool> {
        let v = self.value(value, parent, name)?.to_lowercase();
        match v.as_str() {
            "true" => Ok(true),
            "false" => Ok(false),
            _ => Err(attribute_invalid(&v, parent, name)),
        }
    }
}

/// `String.split(",")`: no separator leaves the string; otherwise
/// trailing empty strings go.
fn java_split(s: &str) -> Vec<&str> {
    if !s.contains(',') {
        return vec![s];
    }
    let mut parts: Vec<&str> = s.split(',').collect();
    while parts.last() == Some(&"") {
        parts.pop();
    }
    parts
}

/// `Integer.parseInt`.
fn parse_int(s: &str) -> Option<i32> {
    let digits = s.strip_prefix(['-', '+']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `BuilderUtils.validateAttribute`: required when unset, prohibited when
/// set to other than the default.
fn validate<T: PartialEq>(
    value: Option<&T>,
    name: &str,
    required: bool,
    prohibited: bool,
    default: Option<&T>,
) -> Result<(), String> {
    if value.is_none() && required {
        return Err(format!("Required attribute {name} missing"));
    }
    if value.is_some() && prohibited && value != default {
        return Err(format!("Prohibited attribute {name} present"));
    }
    Ok(())
}

/// `validateId`.
fn validate_id(id: Option<&String>, name: &str) -> Result<String, String> {
    validate(id, name, true, false, None)?;
    let id = id.unwrap();
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        return Err(format!("Attribute {name} invalid"));
    }
    Ok(id.clone())
}

/// `validateResId`.
fn validate_res(
    value: Option<i32>,
    name: &str,
    required: bool,
    prohibited: bool,
) -> Result<i32, String> {
    validate(value.as_ref(), name, required, prohibited, Some(&ID_NULL))?;
    match value {
        None => Ok(ID_NULL),
        Some(ID_NULL) if required => Err(format!("Required attribute {name} invalid")),
        Some(v) => Ok(v),
    }
}

/// `validateIntDef`.
fn validate_int_def(
    value: Option<i32>,
    name: &str,
    required: bool,
    prohibited: bool,
    default: i32,
    valid: &[i32],
) -> Result<i32, String> {
    validate(value.as_ref(), name, required, prohibited, Some(&default))?;
    match value {
        None => Ok(default),
        Some(v) if valid.contains(&v) => Ok(v),
        Some(_) => Err(format!("Attribute {name} invalid")),
    }
}

/// `validateInteger` and `validateBoolean`.
fn validate_or<T: PartialEq + Copy>(
    value: Option<T>,
    name: &str,
    prohibited: bool,
    default: T,
) -> Result<T, String> {
    validate(value.as_ref(), name, false, prohibited, Some(&default))?;
    Ok(value.unwrap_or(default))
}

#[derive(Default)]
struct SourceBuilder {
    kind: i32,
    id: Option<String>,
    package_name: Option<String>,
    title: Option<i32>,
    title_for_work: Option<i32>,
    summary: Option<i32>,
    intent_action: Option<String>,
    profile: Option<i32>,
    initial_display_state: Option<i32>,
    max_severity_level: Option<i32>,
    search_terms: Option<i32>,
    logging_allowed: Option<bool>,
    refresh_on_page_open_allowed: Option<bool>,
    notifications_allowed: Option<bool>,
    deduplication_group: Option<String>,
    package_certificate_hashes: Vec<String>,
    title_for_private_profile: Option<i32>,
}

impl SourceBuilder {
    /// `SafetySource.Builder.build` on an SDK from Vanilla Ice Cream.
    fn build(self, flags: Flags) -> Result<SafetySource, String> {
        let kind = self.kind;
        if ![TYPE_STATIC, TYPE_DYNAMIC, TYPE_ISSUE_ONLY].contains(&kind) {
            return Err("Unexpected type".into());
        }
        let (is_static, is_dynamic, is_issue_only) = (
            kind == TYPE_STATIC,
            kind == TYPE_DYNAMIC,
            kind == TYPE_ISSUE_ONLY,
        );
        let id = validate_id(self.id.as_ref(), "id")?;
        validate(
            self.package_name.as_ref(),
            "packageName",
            is_dynamic || is_issue_only,
            false,
            None,
        )?;
        let initial_display_state = validate_int_def(
            self.initial_display_state,
            "initialDisplayState",
            false,
            is_static || is_issue_only,
            DISPLAY_ENABLED,
            &[DISPLAY_ENABLED, DISPLAY_DISABLED, DISPLAY_HIDDEN],
        )?;
        let is_enabled = initial_display_state == DISPLAY_ENABLED;
        let is_hidden = initial_display_state == DISPLAY_HIDDEN;
        let is_dynamic_not_hidden = is_dynamic && !is_hidden;
        let profile = validate_int_def(
            self.profile,
            "profile",
            true,
            false,
            PROFILE_NONE,
            &[PROFILE_NONE, PROFILE_PRIMARY, PROFILE_ALL],
        )?;
        let has_all_profiles = profile == PROFILE_ALL;
        let search_terms = validate_res(self.search_terms, "searchTerms", false, is_issue_only)?;
        let is_dynamic_hidden_with_search = is_dynamic && is_hidden && search_terms != ID_NULL;
        let title_required = is_dynamic_not_hidden || is_dynamic_hidden_with_search || is_static;
        let title = validate_res(self.title, "title", title_required, is_issue_only)?;
        let title_for_work = validate_res(
            self.title_for_work,
            "titleForWork",
            has_all_profiles && title_required,
            !has_all_profiles || is_issue_only,
        )?;
        let summary = validate_res(
            self.summary,
            "summary",
            is_dynamic_not_hidden,
            is_issue_only,
        )?;
        validate(
            self.intent_action.as_ref(),
            "intentAction",
            (is_dynamic && is_enabled) || is_static,
            is_issue_only,
            None,
        )?;
        let max_severity_level = validate_or(
            self.max_severity_level,
            "maxSeverityLevel",
            is_static,
            i32::MAX,
        )?;
        let logging_allowed = validate_or(self.logging_allowed, "loggingAllowed", is_static, true)?;
        let refresh_on_page_open_allowed = validate_or(
            self.refresh_on_page_open_allowed,
            "refreshOnPageOpenAllowed",
            is_static,
            false,
        )?;
        let notifications_allowed = validate_or(
            self.notifications_allowed,
            "notificationsAllowed",
            is_static,
            false,
        )?;
        validate(
            self.deduplication_group.as_ref(),
            "deduplicationGroup",
            false,
            is_static,
            None,
        )?;
        if !self.package_certificate_hashes.is_empty() && is_static {
            return Err("Prohibited attribute packageCertificateHashes present".into());
        }
        let title_for_private_profile = if flags.private_profile_title_api {
            validate_res(
                self.title_for_private_profile,
                "titleForPrivateProfile",
                has_all_profiles && title_required,
                !has_all_profiles || is_issue_only,
            )?
        } else {
            ID_NULL
        };
        Ok(SafetySource {
            kind,
            id,
            package_name: self.package_name,
            title,
            title_for_work,
            summary,
            intent_action: self.intent_action,
            profile,
            initial_display_state,
            max_severity_level,
            search_terms,
            logging_allowed,
            refresh_on_page_open_allowed,
            notifications_allowed,
            deduplication_group: self.deduplication_group,
            package_certificate_hashes: self.package_certificate_hashes,
            title_for_private_profile,
        })
    }
}

#[derive(Default)]
struct GroupBuilder {
    kind: Option<i32>,
    id: Option<String>,
    title: Option<i32>,
    summary: Option<i32>,
    stateless_icon_type: Option<i32>,
    sources: Vec<SafetySource>,
}

impl GroupBuilder {
    /// `SafetySourcesGroup.Builder.build`.
    fn build(self) -> Result<SafetySourcesGroup, String> {
        let id = validate_id(self.id.as_ref(), "id")?;
        if self.sources.is_empty() {
            return Err("Safety sources group empty".into());
        }
        let summary = validate_res(self.summary, "summary", false, false)?;
        let stateless_icon_type = validate_int_def(
            self.stateless_icon_type,
            "statelessIconType",
            false,
            false,
            ICON_NONE,
            &[ICON_NONE, ICON_PRIVACY],
        )?;
        let only_issue_only = self.sources.iter().all(|s| s.kind == TYPE_ISSUE_ONLY);
        let inferred = if only_issue_only {
            GROUP_HIDDEN
        } else if summary != ID_NULL || stateless_icon_type != ICON_NONE {
            GROUP_STATEFUL
        } else {
            GROUP_STATELESS
        };
        let kind = validate_int_def(
            self.kind,
            "type",
            false,
            false,
            inferred,
            &[GROUP_STATEFUL, GROUP_STATELESS, GROUP_HIDDEN],
        )?;
        if kind == GROUP_HIDDEN && !only_issue_only {
            return Err(
                "Safety sources groups of type hidden can only contain sources of type issue-only"
                    .into(),
            );
        }
        if kind != GROUP_HIDDEN && only_issue_only {
            return Err("Safety sources groups containing only sources of type issue-only must be of type hidden".into());
        }
        let title = validate_res(
            self.title,
            "title",
            kind == GROUP_STATEFUL || kind == GROUP_STATELESS,
            false,
        )?;
        Ok(SafetySourcesGroup {
            kind,
            id,
            title,
            summary,
            stateless_icon_type,
            sources: self.sources,
        })
    }
}

/// `SafetyCenterConfig.Builder.build`.
fn build_config(groups: Vec<SafetySourcesGroup>) -> Result<SafetyCenterConfig, String> {
    if groups.is_empty() {
        return Err("No safety sources groups present".into());
    }
    let mut group_ids = std::collections::HashSet::new();
    let mut source_ids = std::collections::HashSet::new();
    for g in &groups {
        if !group_ids.insert(&g.id) {
            return Err(format!("Duplicate id {} among safety sources groups", g.id));
        }
        for s in &g.sources {
            if !source_ids.insert(&s.id) {
                return Err(format!("Duplicate id {} among safety sources", s.id));
            }
        }
    }
    Ok(SafetyCenterConfig { groups })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Strings(Vec<(&'static str, u32, &'static str)>);

    impl StringResources for Strings {
        fn identifier(&self, package: &str, kind: &str, entry: &str) -> Option<u32> {
            (package == "p" && kind == "string")
                .then(|| {
                    self.0
                        .iter()
                        .find(|(n, _, _)| *n == entry)
                        .map(|(_, id, _)| *id)
                })
                .flatten()
        }
        fn string(&self, id: u32) -> Result<String, String> {
            self.0
                .iter()
                .find(|(_, i, _)| *i == id)
                .map(|(_, _, s)| s.to_string())
                .ok_or_else(|| format!("no string {id:#x}"))
        }
    }

    const FLAGS: Flags = Flags {
        private_profile_title_api: true,
    };

    fn strings() -> Strings {
        Strings(vec![
            ("t", 0x7f01_0000, "Title"),
            ("s", 0x7f01_0001, "Summary"),
            ("pkg", 0x7f01_0002, "com.example"),
        ])
    }

    /// A stateful group with a dynamic source: the resource names resolve
    /// to ids, a value given as a resource is its string, and the defaults
    /// are the builders'.
    #[test]
    fn a_config_is_parsed_and_built_as_the_original_builds_it() {
        let xml = br#"<!-- c --><safety-center-config>
  <safety-sources-config>
    <safety-sources-group id="G" title="@p:string/t" summary="@p:string/s">
      <dynamic-safety-source id="D" packageName="@p:string/pkg" profile="primary_profile_only"
          intentAction="a" title="@p:string/t" summary="@p:string/s"/>
      <issue-only-safety-source id="I" packageName="x" profile="all_profiles"/>
    </safety-sources-group>
  </safety-sources-config>
</safety-center-config>"#;
        let config = parse(xml, &strings(), FLAGS).unwrap();
        let g = &config.groups[0];
        assert_eq!(
            (g.kind, g.id.as_str(), g.title, g.summary),
            (GROUP_STATEFUL, "G", 0x7f01_0000, 0x7f01_0001)
        );
        let d = &g.sources[0];
        assert_eq!(d.package_name.as_deref(), Some("com.example"));
        assert_eq!(
            (d.kind, d.profile, d.max_severity_level, d.logging_allowed),
            (TYPE_DYNAMIC, PROFILE_PRIMARY, i32::MAX, true)
        );
        assert_eq!(g.sources[1].kind, TYPE_ISSUE_ONLY);
    }

    /// The image's configuration: the Permission APEX's resources APK's
    /// `raw/safety_center_config`, its names resolved in the APK's own
    /// resource table.
    #[test]
    fn the_images_config_parses() {
        use crate::package::parse::resources::{Resources, Table};
        let Some(priv_app) = aim_paths::input(
            aim_paths::out().join("app-host/root/apex/com.android.permission/priv-app"),
            "app-host",
        ) else {
            return;
        };
        let dir = std::fs::read_dir(&priv_app)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("GoogleSafetyCenterResources@")
            })
            .unwrap();
        let apk = aim_apps::apk::Apk::open(&dir.join("GoogleSafetyCenterResources.apk")).unwrap();
        let table = Table::parse(&apk.file_if_present("resources.arsc").unwrap().unwrap()).unwrap();
        struct Apk<'a>(Resources<'a>);
        impl StringResources for Apk<'_> {
            fn identifier(&self, package: &str, kind: &str, entry: &str) -> Option<u32> {
                self.0.identifier(package, kind, entry)
            }
            fn string(&self, id: u32) -> Result<String, String> {
                self.0.resource_string(id).ok_or_else(|| format!("{id:#x}"))
            }
        }
        let base = Resources {
            tables: vec![&table],
            overlays: &[],
            config: Default::default(),
        };
        let id = base
            .identifier(
                "com.google.android.safetycenter.resources",
                "raw",
                "safety_center_config",
            )
            .unwrap();
        let file = |resources: &Resources<'_>| {
            apk.file_if_present(&resources.resource_string(id).unwrap())
                .unwrap()
                .unwrap()
        };
        // The base `raw/safety_center_config.xml` gives no private profile
        // titles, which `private_profile_title_api` (on in the image's flag
        // storage) requires of all-profiles sources.
        let strings = Apk(base);
        assert_eq!(
            parse(&file(&strings.0), &strings, FLAGS).unwrap_err(),
            "Element dynamic-safety-source invalid: Required attribute titleForPrivateProfile missing"
        );
        // The device's SDK picks `raw-v36`, which gives them.
        let device = crate::package::parse::resources::Config {
            sdk_version: 36,
            ..Default::default()
        };
        let strings = Apk(Resources {
            tables: vec![&table],
            overlays: &[],
            config: device,
        });
        let config = parse(&file(&strings.0), &strings, FLAGS).unwrap();
        let ids: Vec<&str> = config.groups.iter().map(|g| g.id.as_str()).collect();
        assert!(ids.contains(&"GoogleAppSecuritySources"), "{ids:?}");
        let protect = config
            .groups
            .iter()
            .flat_map(|g| &g.sources)
            .find(|s| s.id == "GooglePlayProtect")
            .unwrap();
        assert_eq!(
            (
                protect.kind,
                protect.profile,
                protect.package_name.as_deref()
            ),
            (TYPE_DYNAMIC, PROFILE_ALL, Some("com.android.vending"))
        );
        assert_ne!(protect.title_for_private_profile, ID_NULL);
        assert_ne!(protect.title_for_work, ID_NULL);
    }

    /// The parser's and the builders' errors.
    #[test]
    fn invalid_configs_are_refused() {
        let group = |sources: &str| {
            format!(
                r#"<safety-center-config><safety-sources-config><safety-sources-group id="G" title="@p:string/t">{sources}</safety-sources-group></safety-sources-config></safety-center-config>"#
            )
        };
        let fails = |xml: &str| parse(xml.as_bytes(), &strings(), FLAGS).unwrap_err();
        assert_eq!(fails("<other/>"), "Element safety-center-config missing");
        assert_eq!(
            fails(r#"<safety-center-config a="1"/>"#),
            "Element safety-center-config invalid"
        );
        assert_eq!(
            fails("<safety-center-config><safety-sources-config/></safety-center-config>"),
            "Element safety-sources-config invalid: No safety sources groups present"
        );
        assert_eq!(
            fails(&group("")),
            "Element safety-sources-group invalid: Safety sources group empty"
        );
        assert_eq!(
            fails(&group(r#"<dynamic-safety-source id="D" packageName="x"/>"#)),
            "Element dynamic-safety-source invalid: Required attribute profile missing"
        );
        assert_eq!(
            fails(&group(
                r#"<static-safety-source id="S" profile="all_profiles" title="@p:string/t" intentAction="a" loggingAllowed="false"/>"#
            )),
            "Element static-safety-source invalid: Required attribute titleForWork missing"
        );
        assert_eq!(
            fails(&group(r#"<static-safety-source id="S" foo="1"/>"#)),
            "Unexpected attribute static-safety-source.foo"
        );
        assert_eq!(
            fails(&group(
                r#"<dynamic-safety-source id="D" packageName="x" profile="all" />"#
            )),
            "Attribute value \"all\" in dynamic-safety-source.profile invalid"
        );
        assert_eq!(
            fails(&group(
                r#"<dynamic-safety-source id="D" packageName="x" profile="primary_profile_only" title="@p:string/missing"/>"#
            )),
            "Resource name \"@p:string/missing\" in dynamic-safety-source.title missing or invalid"
        );
        assert_eq!(
            fails(&group(
                r#"<issue-only-safety-source id="I" packageName="x" profile="primary_profile_only"/><other/>"#
            )),
            "Element safety-sources-group not closed"
        );
        let source =
            r#"<issue-only-safety-source id="I" packageName="x" profile="primary_profile_only"/>"#;
        let twice = format!(
            r#"<safety-center-config><safety-sources-config><safety-sources-group id="G" type="hidden">{source}</safety-sources-group><safety-sources-group id="G" type="hidden">{source}</safety-sources-group></safety-sources-config></safety-center-config>"#
        );
        assert_eq!(
            fails(&twice),
            "Element safety-sources-config invalid: Duplicate id G among safety sources groups"
        );
    }
}
