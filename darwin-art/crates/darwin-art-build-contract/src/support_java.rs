//! Production Java manifest contract shared by compiler and graph emitter.
use std::{
    collections::BTreeSet,
    path::{Component, PathBuf},
};

pub const SOURCE_MANIFEST: &str = "runtime/framework/support-sources.txt";

pub fn production_sources(manifest: &str) -> Result<Vec<PathBuf>, String> {
    let mut seen = BTreeSet::new();
    let mut sources = Vec::new();
    for line in manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
    {
        let path = PathBuf::from(line);
        if !line.starts_with("runtime/framework/")
            || !line.ends_with(".java")
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || line.contains("/compile-stubs/")
            || line
                .split('/')
                .any(|component| component.eq_ignore_ascii_case("probes"))
        {
            return Err(format!("invalid production Java source: {line}"));
        }
        if !seen.insert(path.clone()) {
            return Err(format!("duplicate production Java source: {line}"));
        }
        sources.push(path);
    }
    if sources.is_empty() {
        return Err("empty production Java manifest".into());
    }
    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::production_sources;

    #[test]
    fn fixture_directories_cannot_be_promoted_into_production_manifest() {
        for source in [
            "probes/Fixture.java",
            "runtime/framework/probes/Fixture.java",
            "runtime/framework/wm/probes/Fixture.java",
            "runtime/framework/wm/Probes/Fixture.java",
        ] {
            assert!(production_sources(source).is_err(), "accepted {source}");
        }
        assert!(production_sources("runtime/framework/wm/WindowManagerEndpoint.java").is_ok());
    }
}
