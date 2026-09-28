use super::*;
use crate::command::Kind;
impl Registry {
    pub(in crate::connection_registry) fn execute_context_reference(
        &self,
        key: &Key,
        input: &[u8],
        kind: Kind,
    ) -> Result<session::Outcome, super::super::Error> {
        use super::super::Error as RegistryError;
        let sender = self.lookup(key)?;
        if !matches!(kind, Kind::Increfs | Kind::Acquire) {
            return sender
                .execute_context_reference(input, None)
                .map_err(RegistryError::Connection);
        }
        let context = self.context.lock().map_err(|_| RegistryError::Poisoned)?;
        let manager = if let Some(manager) = &context.manager {
            if manager.key.id == key.id {
                return Err(RegistryError::ContextSelfAcquire);
            }
            Some(Arc::clone(manager._references.node()))
        } else {
            None
        };
        // Keep manager generation stable until counted in the sender table.
        sender
            .execute_context_reference(input, manager)
            .map_err(RegistryError::Connection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        objects,
        reference_table::{Counts, Error as RefError},
        session::Session,
    };
    fn open(registry: &Registry) -> Key {
        registry
            .register(ConnectionOwner::new(Session::default()))
            .unwrap()
    }
    fn register(registry: &Registry, key: &Key) {
        let mut bytes = [0; 24];
        bytes[..4].copy_from_slice(&objects::Kind::Binder.tag().to_le_bytes());
        let objects = objects::validate(&bytes, &0u64.to_le_bytes()).unwrap();
        registry
            .register_context_manager(key, &objects[0], |_| Ok(1000))
            .unwrap();
    }
    fn execute(registry: &Registry, key: &Key, kind: Kind) -> session::context_reference::Outcome {
        let bytes = [kind.word().to_le_bytes(), 0u32.to_le_bytes()].concat();
        let session::Outcome::ContextReference(outcome) = registry.execute(key, &bytes).unwrap()
        else {
            panic!("wrong dispatch")
        };
        assert_eq!(outcome.bytes, 8);
        outcome
    }
    #[test]
    fn acquisition_promotion_release_and_manager_absence() {
        let registry = Registry::default();
        let sender = open(&registry);
        assert_eq!(
            execute(&registry, &sender, Kind::Acquire).update,
            Err(RefError::UnknownHandle)
        );
        let manager = open(&registry);
        register(&registry, &manager);
        assert_eq!(
            execute(&registry, &sender, Kind::Increfs).update.unwrap(),
            (0, Counts::default(), Counts { strong: 0, weak: 1 })
        );
        assert_eq!(
            execute(&registry, &sender, Kind::Acquire).update.unwrap().2,
            Counts { strong: 1, weak: 1 }
        );
        registry.close(&manager).unwrap();
        assert_eq!(
            execute(&registry, &sender, Kind::Release).update.unwrap().2,
            Counts { strong: 0, weak: 1 }
        );
        assert_eq!(
            execute(&registry, &sender, Kind::Decrefs).update.unwrap().2,
            Counts::default()
        );
        assert_eq!(
            execute(&registry, &sender, Kind::Acquire).update,
            Err(RefError::UnknownHandle)
        );
    }
    #[test]
    fn replacement_reports_actual_descriptor_and_self_acquire_fails() {
        let registry = Registry::default();
        let sender = open(&registry);
        let old = open(&registry);
        register(&registry, &old);
        execute(&registry, &sender, Kind::Increfs).update.unwrap();
        let command = [Kind::Acquire.word().to_le_bytes(), 0u32.to_le_bytes()].concat();
        assert_eq!(
            registry.execute(&old, &command),
            Err(crate::connection_registry::Error::ContextSelfAcquire)
        );
        registry.close(&old).unwrap();
        let new = open(&registry);
        register(&registry, &new);
        assert_eq!(
            execute(&registry, &sender, Kind::Acquire).update.unwrap().0,
            1
        );
        assert_eq!(
            execute(&registry, &sender, Kind::Decrefs).update.unwrap().2,
            Counts::default()
        );
        // Identity stays at1 even though0 is now free.
        assert_eq!(
            execute(&registry, &sender, Kind::Acquire).update.unwrap().0,
            1
        );
    }
}
