//! What PackageManager keeps per user about each package
//! (`/data/system/users/0/package-restrictions.xml`): whether the package
//! is installed and enabled for the user, and the components enabled or
//! disabled at run time. Android writes it in its binary XML ("ABX",
//! `BinaryXmlSerializer`), and reads text XML too.

use std::collections::HashMap;
use std::path::Path;

/// `PackageManager.COMPONENT_ENABLED_STATE_*` that switch a package off.
const DISABLED_STATES: [i32; 3] = [2, 3, 4];

/// One package's state for the user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Package {
    pub installed: bool,
    pub enabled: bool,
    pub enabled_components: Vec<String>,
    pub disabled_components: Vec<String>,
}

/// The packages' states, by package name.
pub type Restrictions = HashMap<String, Package>;

/// Read `path`; empty when it is missing or unreadable (every package
/// installed and enabled, as PackageManager assumes then).
pub fn read(path: &Path) -> Restrictions {
    std::fs::read(path)
        .ok()
        .and_then(|b| parse(&b))
        .unwrap_or_default()
}

/// Parse a document (binary or text XML) into package states.
fn parse(b: &[u8]) -> Option<Restrictions> {
    let root = aim_android_xml::read(b).ok()?;
    let mut out = Restrictions::new();
    for pkg in root.children().filter(|e| e.name == "pkg") {
        let Some(name) = pkg.string("name") else {
            continue;
        };
        let components = |list: &str| {
            pkg.children()
                .filter(|c| c.name == list)
                .flat_map(|c| c.children())
                .filter_map(|item| item.string("name").map(|n| n.into_owned()))
                .collect()
        };
        let package = Package {
            installed: pkg.bool("inst").ok()?.unwrap_or(true),
            enabled: !DISABLED_STATES.contains(&pkg.int("enabled").ok()?.unwrap_or(0)),
            enabled_components: components("enabled-components"),
            disabled_components: components("disabled-components"),
        };
        out.insert(name.into_owned(), package);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aim_android_xml::{Element, Node, Value, abx};

    fn element(name: &str, attrs: Vec<(&str, Value)>, children: Vec<Element>) -> Element {
        Element {
            name: name.into(),
            attrs: attrs.into_iter().map(|(n, v)| (n.into(), v)).collect(),
            content: children.into_iter().map(Node::Element).collect(),
        }
    }

    #[test]
    fn packages_and_components() {
        let name = |n: &str| ("name", Value::String(n.into()));
        let root = element(
            "package-restrictions",
            Vec::new(),
            vec![
                element(
                    "pkg",
                    vec![name("org.gone"), ("inst", Value::Bool(false))],
                    Vec::new(),
                ),
                element(
                    "pkg",
                    vec![name("org.app"), ("enabled", Value::Int(1))],
                    vec![element(
                        "disabled-components",
                        Vec::new(),
                        vec![element("item", vec![name("org.app.Launcher")], Vec::new())],
                    )],
                ),
                element(
                    "pkg",
                    vec![name("org.off"), ("enabled", Value::Int(3))],
                    Vec::new(),
                ),
            ],
        );
        let r = parse(&abx::write(&root).unwrap()).unwrap();
        assert!(!r["org.gone"].installed);
        assert_eq!(r["org.app"].disabled_components, ["org.app.Launcher"]);
        assert!(r["org.app"].enabled && r["org.app"].installed);
        assert!(!r["org.off"].enabled);
        assert_eq!(parse(b"<?xml"), None);
    }
}
