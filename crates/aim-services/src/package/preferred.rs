//! A user's preferred and persistent preferred activities
//! (`PreferredActivity`, `PreferredComponent`, `PersistentPreferredActivity`
//! and their `IntentResolver`s at `android-16.0.0_r1`), read from the XML
//! the original writes them in: `getPreferredActivityBackup`'s
//! `<preferred-activities>` and package-restrictions.xml's
//! `<persistent-preferred-activities>`.

use aim_android_xml::{Element, Node};

use super::intent::{ComponentName, Intent};
use super::intent_filter::IntentFilter;
use super::intent_resolver::{Build, Entry, IntentResolver};

/// `ComponentName.unflattenFromString`: `package/class`, a class starting
/// with `.` relative to the package.
pub fn unflatten(s: &str) -> Option<ComponentName> {
    let (package, class) = s.split_once('/')?;
    if class.is_empty() {
        return None;
    }
    let class = if class.starts_with('.') {
        format!("{package}{class}")
    } else {
        class.to_owned()
    };
    Some(ComponentName {
        package: package.to_owned(),
        class,
    })
}

/// `PreferredActivity`: a filter and the component chosen for it.
#[derive(Clone, Debug, PartialEq)]
pub struct PreferredActivity {
    pub filter: IntentFilter,
    pub component: ComponentName,
    /// The match quality of the choice (`MATCH_CATEGORY_*`).
    pub match_: i32,
    pub always: bool,
    /// The components offered when the choice was made.
    pub set: Option<Vec<ComponentName>>,
}

impl Entry for PreferredActivity {
    fn filter(&self) -> &IntentFilter {
        &self.filter
    }
    fn package(&self) -> &str {
        &self.component.package
    }
}

/// `PersistentPreferredActivity`.
#[derive(Clone, Debug, PartialEq)]
pub struct PersistentPreferredActivity {
    pub filter: IntentFilter,
    pub component: ComponentName,
    pub set_by_dpm: bool,
}

impl Entry for PersistentPreferredActivity {
    fn filter(&self) -> &IntentFilter {
        &self.filter
    }
    fn package(&self) -> &str {
        &self.component.package
    }
}

/// A user's preferred activities in the original's add order.
#[derive(Clone, Debug, Default)]
pub struct Preferred {
    pub preferred: IntentResolver<PreferredActivity>,
    pub persistent: IntentResolver<PersistentPreferredActivity>,
}

/// `PreferredComponent(parser)`; `None` for an entry with a parse error,
/// which the original drops.
fn preferred_activity(item: &Element) -> Option<PreferredActivity> {
    let component = unflatten(&item.string("name")?)?;
    let match_ = item.int_hex("match").ok().flatten().unwrap_or(0);
    let count = item.int("set").ok().flatten().unwrap_or(0);
    let always = item.bool("always").ok().flatten().unwrap_or(true);
    let mut set = Vec::new();
    let mut filter = IntentFilter::default();
    for c in item.children() {
        match c.name.as_str() {
            "set" => set.push(unflatten(&c.string("name")?)?),
            "filter" => filter = IntentFilter::parse(c),
            _ => {}
        }
    }
    if set.len() != count.max(0) as usize {
        return None;
    }
    Some(PreferredActivity {
        filter,
        component,
        match_,
        always,
        set: (count > 0).then_some(set),
    })
}

/// Reading a valid preferred activity creates a resolver, even if it is later
/// emptied. Empty or invalid-only documents do not create one.
pub(crate) fn has_preferred_resolver(root: &Element) -> bool {
    items(root, "preferred-activities")
        .into_iter()
        .any(|item| preferred_activity(item).is_some())
}

/// Settings.clearPackagePreferredActivities for one user's saved resolver.
/// A named package clears only always choices; null clears every choice.
/// Persistent choices, candidate sets and unrelated XML remain untouched.
pub(crate) fn clear_package_document(root: &mut Element, package: Option<&str>) -> bool {
    let mut changed = false;
    for node in &mut root.content {
        let Node::Element(list) = node else { continue };
        if list.name != "preferred-activities" {
            continue;
        }
        list.content.retain(|node| {
            let Node::Element(item) = node else {
                return true;
            };
            let remove = item.name == "item"
                && preferred_activity(item).is_some_and(|activity| {
                    package.is_none_or(|name| activity.always && activity.component.package == name)
                });
            changed |= remove;
            !remove
        });
    }
    changed
}

fn persistent_preferred_activity(item: &Element) -> Option<PersistentPreferredActivity> {
    Some(PersistentPreferredActivity {
        component: unflatten(&item.string("name")?)?,
        set_by_dpm: item.bool("set-by-dpm").ok().flatten().unwrap_or(false),
        filter: item
            .children()
            .find(|c| c.name == "filter")
            .map(IntentFilter::parse)
            .unwrap_or_default(),
    })
}

/// The `<item>`s of the first element named `name` at or below `e`.
fn items<'a>(e: &'a Element, name: &str) -> Vec<&'a Element> {
    if e.name == name {
        return e.children().filter(|c| c.name == "item").collect();
    }
    e.children().flat_map(|c| items(c, name)).collect()
}

impl Preferred {
    /// From the backup of the preferred activities and package
    /// restrictions; either may be missing or unreadable (then none of
    /// its activities).
    pub fn parse(backup: Option<&[u8]>, restrictions: Option<&[u8]>) -> Preferred {
        let mut p = Preferred::default();
        let read = |b: Option<&[u8]>| b.and_then(|b| aim_android_xml::read(b).ok());
        if let Some(root) = read(backup) {
            for item in items(&root, "preferred-activities") {
                if let Some(pa) = preferred_activity(item) {
                    p.preferred.add(pa);
                }
            }
        }
        if let Some(root) = read(restrictions) {
            for item in items(&root, "persistent-preferred-activities") {
                if let Some(ppa) = persistent_preferred_activity(item) {
                    p.persistent.add(ppa);
                }
            }
        }
        p
    }
}

/// The entries an `IntentResolver` of the default kind returns (the
/// filters themselves), sorted by priority as its `sortResults`.
struct Itself;

impl<E: Entry + Clone> Build<E, E> for Itself {
    fn result(&mut self, e: &E, _: i32) -> Option<E> {
        Some(e.clone())
    }
}

/// `queryIntent` of a resolver whose results are its filters.
pub fn query<E: Entry + Clone>(
    r: &IntentResolver<E>,
    intent: &Intent,
    resolved_type: Option<&str>,
    default_only: bool,
) -> std::result::Result<Vec<E>, super::domain_verification::uri_parcel::MatchError> {
    let mut list = r.query(intent, resolved_type, default_only, &mut Itself)?;
    list.sort_by(|a, b| b.filter().priority.cmp(&a.filter().priority));
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BACKUP: &[u8] = br#"<?xml version='1.0' encoding='utf-8' standalone='yes' ?>
<preferred-backup>
<preferred-activities>
<item name="com.example.browser/.Main" match="200000" always="true" set="2">
<set name="com.example.browser/.Main" />
<set name="com.other/com.other.View" />
<filter>
<action name="android.intent.action.VIEW" />
<cat name="android.intent.category.DEFAULT" />
<scheme name="https" />
</filter>
</item>
<item name="com.broken/.X" set="1">
<filter />
</item>
</preferred-activities>
</preferred-backup>"#;

    const RESTRICTIONS: &[u8] = br#"<package-restrictions>
<persistent-preferred-activities>
<item name="com.example.camera/com.example.camera.Shoot" set-by-dpm="true">
<filter>
<action name="android.media.action.IMAGE_CAPTURE" />
<cat name="android.intent.category.DEFAULT" />
</filter>
</item>
</persistent-preferred-activities>
</package-restrictions>"#;

    #[test]
    fn reads_and_queries() {
        let p = Preferred::parse(Some(BACKUP), Some(RESTRICTIONS));
        // The broken item (one set entry declared, none given) is dropped.
        assert_eq!(p.preferred.entries().len(), 1);
        let pa = &p.preferred.entries()[0];
        assert_eq!(pa.component.class, "com.example.browser.Main");
        assert_eq!((pa.match_, pa.always), (0x200000, true));
        assert_eq!(pa.set.as_ref().unwrap()[1].class, "com.other.View");
        let view = Intent {
            action: Some("android.intent.action.VIEW".into()),
            data: Some(super::super::uri::Uri::parse("https://example.com/")),
            ..Intent::default()
        };
        assert_eq!(query(&p.preferred, &view, None, true).unwrap().len(), 1);
        let ppa = &p.persistent.entries()[0];
        assert!(ppa.set_by_dpm);
        let capture = Intent {
            action: Some("android.media.action.IMAGE_CAPTURE".into()),
            ..Intent::default()
        };
        assert_eq!(query(&p.persistent, &capture, None, true).unwrap().len(), 1);
        assert!(
            Preferred::parse(None, Some(b"not xml"))
                .persistent
                .entries()
                .is_empty()
        );
    }

    #[test]
    fn unflattens() {
        assert_eq!(unflatten("a/.B").unwrap().class, "a.B");
        assert_eq!(unflatten("a/b.C").unwrap().class, "b.C");
        assert!(unflatten("a/").is_none() && unflatten("a").is_none());
    }
}
