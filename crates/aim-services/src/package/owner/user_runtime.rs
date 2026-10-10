//! PackageUserStateImpl runtime owners, Android 16.0.0_r1.
// Portions ported from AOSP PackageUserStateImpl and OverlayPaths.
// Copyright (C) The Android Open Source Project, Apache License 2.0.
use crate::package::model::OverlayPaths;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    overlays: Option<OverlayPaths>,
    libraries: Option<Vec<(String, OverlayPaths)>>,
    overrides: Option<Vec<(Component, LabelIcon)>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Component {
    pub package: String,
    pub class: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LabelIcon {
    pub label: Option<String>,
    pub icon: Option<i32>,
}
fn hash(s: &str) -> i32 {
    s.encode_utf16()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
}
fn empty(paths: &OverlayPaths) -> bool {
    paths.resource_dirs.is_empty() && paths.overlay_paths.is_empty()
}
impl State {
    pub fn overlays(&self) -> Option<&OverlayPaths> {
        self.overlays.as_ref()
    }
    pub fn libraries(&self) -> Option<&[(String, OverlayPaths)]> {
        self.libraries.as_deref()
    }
    pub fn overrides(&self) -> Option<&[(Component, LabelIcon)]> {
        self.overrides.as_deref()
    }
    pub fn set_overlay_paths(&mut self, paths: Option<OverlayPaths>) -> bool {
        if self.overlays == paths
            || (self.overlays.is_none() && paths.as_ref().is_some_and(empty))
            || (paths.is_none() && self.overlays.as_ref().is_some_and(empty))
        {
            return false;
        }
        self.overlays = paths;
        true
    }
    pub fn set_library_overlay_paths(
        &mut self,
        library: String,
        paths: Option<OverlayPaths>,
    ) -> bool {
        let libraries = self.libraries.get_or_insert_with(Vec::new);
        let at = libraries.iter().position(|(name, _)| name == &library);
        if at.map(|i| &libraries[i].1) == paths.as_ref() {
            return false;
        }
        if paths.as_ref().is_none_or(empty) {
            if let Some(i) = at {
                libraries.remove(i);
                return true;
            }
            return false;
        }
        let paths = paths.unwrap();
        if let Some(i) = at {
            libraries[i].1 = paths;
        } else {
            libraries.push((library, paths));
            libraries.sort_by_key(|(name, _)| hash(name));
        }
        true
    }
    pub fn all_overlay_paths(&self) -> Option<OverlayPaths> {
        if self.overlays.is_none() && self.libraries.is_none() {
            return None;
        }
        // Builder(base) retains base duplicates; addAll deduplicates later paths.
        let mut paths = self.overlays.clone().unwrap_or_default();
        for (_, library) in self.libraries.iter().flatten() {
            for (target, source) in [
                (&mut paths.resource_dirs, &library.resource_dirs),
                (&mut paths.overlay_paths, &library.overlay_paths),
            ] {
                for value in source {
                    if !target.contains(value) {
                        target.push(value.clone());
                    }
                }
            }
        }
        Some(paths)
    }
    pub fn override_label_icon(&mut self, component: Component, value: LabelIcon) -> bool {
        let at = self
            .overrides
            .as_ref()
            .and_then(|entries| entries.iter().position(|(key, _)| key == &component));
        let old = at
            .map(|i| self.overrides.as_ref().unwrap()[i].1.clone())
            .unwrap_or_default();
        if old == value {
            return false;
        }
        if value == LabelIcon::default() {
            let entries = self.overrides.as_mut().unwrap();
            entries.remove(at.unwrap());
            if entries.is_empty() {
                self.overrides = None;
            }
        } else {
            let entries = self.overrides.get_or_insert_with(Vec::new);
            if let Some(i) = at {
                entries[i].1 = value;
            } else {
                entries.push((component, value));
                entries.sort_by_key(|(c, _)| hash(&c.package).wrapping_add(hash(&c.class)));
            }
        }
        true
    }
    pub fn reset_label_icons(&mut self) {
        self.overrides = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn paths(names: &[&str]) -> OverlayPaths {
        OverlayPaths {
            resource_dirs: names.iter().map(|s| s.to_string()).collect(),
            overlay_paths: names.iter().map(|s| s.to_string()).collect(),
        }
    }
    #[test]
    fn empty_library_allocation_and_merge_keep_original_defaults_and_order() {
        let mut state = State::default();
        assert_eq!(state.all_overlay_paths(), None);
        assert!(!state.set_overlay_paths(Some(paths(&[]))));
        assert!(!state.set_library_overlay_paths("missing".into(), None));
        assert_eq!(state.all_overlay_paths(), Some(paths(&[])));
        assert!(state.set_overlay_paths(Some(paths(&["base", "base"]))));
        assert!(state.set_library_overlay_paths("Aa".into(), Some(paths(&["one", "base"]))));
        assert!(state.set_library_overlay_paths("B".into(), Some(paths(&["two"]))));
        assert!(state.set_library_overlay_paths("BB".into(), Some(paths(&["three"]))));
        assert_eq!(
            state.all_overlay_paths(),
            Some(paths(&["base", "base", "two", "one", "three"]))
        );
        let captured = state.clone();
        assert!(state.set_library_overlay_paths("B".into(), None));
        assert!(state.set_overlay_paths(None));
        assert_eq!(
            captured.all_overlay_paths(),
            Some(paths(&["base", "base", "two", "one", "three"]))
        );
    }
    #[test]
    fn label_icon_owners_distinguish_removal_zero_and_empty_label() {
        let mut state = State::default();
        let c = Component {
            package: "fixture".into(),
            class: "Activity".into(),
        };
        assert!(!state.override_label_icon(c.clone(), LabelIcon::default()));
        let value = LabelIcon {
            label: Some(String::new()),
            icon: Some(0),
        };
        assert!(state.override_label_icon(c.clone(), value.clone()));
        assert!(!state.override_label_icon(c.clone(), value));
        let captured = state.clone();
        assert!(state.override_label_icon(c, LabelIcon::default()));
        assert!(state.overrides().is_none());
        assert_eq!(captured.overrides().unwrap()[0].1.icon, Some(0));
    }
}
crate::install_mutator_user_runtime_restore!();
