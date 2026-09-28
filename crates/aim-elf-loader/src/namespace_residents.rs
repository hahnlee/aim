//! Edges of already admitted, already loaded images. No path or symbol grants.
use super::*;

impl ClosedElfNamespace {
    /// Register an admitted resident and its original ordered dependencies.
    /// The external resolver handles its selected exports; native_owners must
    /// retain its real image. Dependency edges never promote it to global.
    pub fn add_provider_with_dependencies(
        &mut self,
        soname: impl Into<String>,
        needed: Vec<String>,
    ) -> Result<(), NamespaceError> {
        let soname = soname.into();
        if needed.iter().any(|name| name.is_empty()) {
            return Err(NamespaceError::UnknownDependency {
                requested_by: soname,
                soname: String::new(),
            });
        }
        self.add_provider(soname.clone())?;
        self.resident_dependencies.insert(soname, needed);
        Ok(())
    }

    pub(super) fn validate_resident_dependencies(
        &self,
        globals: &[GlobalElfImage],
    ) -> Result<(), NamespaceError> {
        for (parent, needed) in &self.resident_dependencies {
            for name in needed {
                // A previously loaded object's dependencies must already be
                // retained/admitted, not rebound to a newly opened byte source.
                if !self.providers.contains(name) && !globals.iter().any(|g| &g.soname == name) {
                    return Err(NamespaceError::UnknownDependency {
                        requested_by: parent.clone(),
                        soname: name.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resident_edges_require_admitted_owners_not_new_files() {
        let mut ns = ClosedElfNamespace::new();
        ns.add_provider_with_dependencies("a", vec!["b".into()])
            .unwrap();
        assert!(ns.validate_resident_dependencies(&[]).is_err());
        ns.add_elf("b", vec![]).unwrap();
        assert!(ns.validate_resident_dependencies(&[]).is_err());
        let mut ns = ClosedElfNamespace::new();
        ns.add_provider_with_dependencies("a", vec!["b".into()])
            .unwrap();
        ns.add_provider_with_dependencies("b", vec!["a".into()])
            .unwrap();
        assert!(ns.validate_resident_dependencies(&[]).is_ok());
        assert!(ns.add_provider_with_dependencies("a", vec![]).is_err());
        assert_eq!(ns.resident_dependencies["a"], ["b"]);
    }
}
