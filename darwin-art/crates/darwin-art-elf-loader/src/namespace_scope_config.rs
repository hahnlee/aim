//! Explicit linker-owner input. This carries admission decisions, not path
//! search or permission policy; absent metadata must not mean global access.
use super::*;

#[derive(Clone, Default)]
pub struct NamespaceScopes {
    pub primary: HashMap<String, u64>,
    pub accessible: HashMap<u64, HashSet<String>>,
    /// Ordered indices into the supplied retained global-image slice.
    pub globals: HashMap<u64, Vec<usize>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_scopes_require_complete_valid_metadata() {
        let names = vec!["root".to_owned()];
        let mut scopes = NamespaceScopes::default();
        assert!(scopes.validate(&names, 0).is_err());
        scopes.primary.insert("root".into(), 10);
        assert!(scopes.validate(&names, 0).is_err());
        scopes.accessible.insert(10, HashSet::from(["root".into()]));
        assert!(scopes.validate(&names, 0).is_err());
        scopes.globals.insert(10, vec![0]);
        assert!(scopes.validate(&names, 0).is_err());
        assert!(scopes.validate(&names, 1).is_ok());
        scopes.globals.insert(10, vec![]);
        assert!(scopes.validate(&names, 0).is_ok());
    }
}
impl NamespaceScopes {
    pub(super) fn namespace(&self, name: &str) -> Result<u64, NamespaceError> {
        self.primary
            .get(name)
            .copied()
            .filter(|id| *id != 0)
            .ok_or(NamespaceError::Scope("missing primary namespace"))
    }
    pub(super) fn permits(&self, namespace: u64, name: &str) -> bool {
        self.accessible
            .get(&namespace)
            .is_some_and(|names| names.contains(name))
    }
    pub(crate) fn validate(
        &self,
        names: &[String],
        global_count: usize,
    ) -> Result<(), NamespaceError> {
        for name in names {
            let namespace = self.namespace(name)?;
            if !self.permits(namespace, name) || !self.globals.contains_key(&namespace) {
                return Err(NamespaceError::Scope(
                    "missing namespace visibility/global scope",
                ));
            }
        }
        if self
            .globals
            .values()
            .flatten()
            .any(|&index| index >= global_count)
        {
            return Err(NamespaceError::Scope("invalid global image index"));
        }
        Ok(())
    }
}
