//! Concrete Rust ownership slots for the native runtime resources.
//!
//! `RuntimeSession` tracks phase/lease order.  This type owns the actual
//! engine, provider, and optional surface values so production code does not
//! need to hide them behind `Any` just to get reverse teardown.

use std::marker::PhantomData;
use std::rc::Rc;

/// Concrete resources owned by one owner thread.
///
/// Field order is intentional: if a caller forgets an explicit shutdown,
/// graphics drops before surface, surface before provider, and provider before
/// engine. Normal teardown should still call the resource-specific close methods before
/// taking the values out of these slots so failures remain observable.
pub(crate) struct RuntimeOwners<E, P, S, G = ()> {
    graphics: Option<G>,
    surface: Option<S>,
    provider: Option<P>,
    engine: Option<E>,
    // Native callbacks and surface handles are owner-thread resources.  The
    // marker makes accidental cross-thread moves a compile-time error instead
    // of relying on every caller to remember the ART affinity rule.
    _owner_thread: PhantomData<Rc<()>>,
}

impl<E, P, S, G> Drop for RuntimeOwners<E, P, S, G> {
    fn drop(&mut self) {
        // Keep rollback order explicit at the owner boundary.  The native
        // session values each have their own close contract; this only
        // determines which owner is dropped first when a caller returns
        // before the lease-driven teardown path can run.
        drop(self.graphics.take());
        drop(self.surface.take());
        drop(self.provider.take());
        drop(self.engine.take());
    }
}

impl<E, P, S, G> RuntimeOwners<E, P, S, G> {
    pub const fn new() -> Self {
        Self {
            graphics: None,
            surface: None,
            provider: None,
            engine: None,
            _owner_thread: PhantomData,
        }
    }

    pub fn attach_engine(&mut self, engine: E) -> Result<(), E> {
        if self.engine.is_some() {
            Err(engine)
        } else {
            self.engine = Some(engine);
            Ok(())
        }
    }

    pub fn attach_provider(&mut self, provider: P) -> Result<(), P> {
        if self.provider.is_some() {
            Err(provider)
        } else {
            self.provider = Some(provider);
            Ok(())
        }
    }

    pub fn attach_surface(&mut self, surface: S) -> Result<(), S> {
        if self.surface.is_some() {
            Err(surface)
        } else {
            self.surface = Some(surface);
            Ok(())
        }
    }

    pub fn attach_graphics(&mut self, graphics: G) -> Result<(), G> {
        if self.graphics.is_some() {
            Err(graphics)
        } else {
            self.graphics = Some(graphics);
            Ok(())
        }
    }

    pub fn engine(&self) -> Option<&E> {
        self.engine.as_ref()
    }

    pub(crate) fn engine_mut(&mut self) -> Option<&mut E> {
        self.engine.as_mut()
    }

    pub fn provider(&self) -> Option<&P> {
        self.provider.as_ref()
    }

    pub(crate) fn provider_mut(&mut self) -> Option<&mut P> {
        self.provider.as_mut()
    }

    pub fn surface(&self) -> Option<&S> {
        self.surface.as_ref()
    }

    pub(crate) fn surface_mut(&mut self) -> Option<&mut S> {
        self.surface.as_mut()
    }

    pub fn graphics(&self) -> Option<&G> {
        self.graphics.as_ref()
    }

    pub fn graphics_mut(&mut self) -> Option<&mut G> {
        self.graphics.as_mut()
    }

    pub(crate) fn take_engine(&mut self) -> Option<E> {
        self.engine.take()
    }

    pub(crate) fn take_provider(&mut self) -> Option<P> {
        self.provider.take()
    }

    pub(crate) fn take_surface(&mut self) -> Option<S> {
        self.surface.take()
    }

    pub(crate) fn take_graphics(&mut self) -> Option<G> {
        self.graphics.take()
    }

    pub fn is_empty(&self) -> bool {
        self.engine.is_none()
            && self.provider.is_none()
            && self.surface.is_none()
            && self.graphics.is_none()
    }
}

impl<E, P, S, G> Default for RuntimeOwners<E, P, S, G> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::RuntimeOwners;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Debug)]
    struct DropProbe {
        name: &'static str,
        order: Rc<RefCell<Vec<&'static str>>>,
    }

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.order.borrow_mut().push(self.name);
        }
    }

    #[test]
    fn slots_reject_duplicates_and_preserve_concrete_values() {
        let mut owners = RuntimeOwners::<u8, u16, u32>::new();
        assert!(owners.attach_engine(1).is_ok());
        assert_eq!(owners.attach_engine(2), Err(2));
        assert!(owners.attach_provider(3).is_ok());
        assert!(owners.attach_surface(4).is_ok());
        assert_eq!(owners.engine(), Some(&1));
        assert_eq!(owners.provider(), Some(&3));
        assert_eq!(owners.surface(), Some(&4));
        assert_eq!(owners.take_surface(), Some(4));
        assert_eq!(owners.take_provider(), Some(3));
        assert_eq!(owners.take_engine(), Some(1));
        assert!(owners.is_empty());
    }

    #[test]
    fn implicit_drop_preserves_surface_provider_engine_order() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let mut owners = RuntimeOwners::<DropProbe, DropProbe, DropProbe, ()>::new();
        owners
            .attach_engine(DropProbe {
                name: "engine",
                order: Rc::clone(&order),
            })
            .unwrap();
        owners
            .attach_provider(DropProbe {
                name: "provider",
                order: Rc::clone(&order),
            })
            .unwrap();
        owners
            .attach_surface(DropProbe {
                name: "surface",
                order: Rc::clone(&order),
            })
            .unwrap();
        drop(owners);
        assert_eq!(&*order.borrow(), &["surface", "provider", "engine"]);
    }

    #[test]
    fn implicit_drop_releases_graphics_before_surface_provider_engine() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let mut owners = RuntimeOwners::<DropProbe, DropProbe, DropProbe, DropProbe>::new();
        for name in ["engine", "provider", "surface", "graphics"] {
            let probe = DropProbe {
                name,
                order: Rc::clone(&order),
            };
            match name {
                "engine" => owners.attach_engine(probe).unwrap(),
                "provider" => owners.attach_provider(probe).unwrap(),
                "surface" => owners.attach_surface(probe).unwrap(),
                _ => owners.attach_graphics(probe).unwrap(),
            }
        }
        drop(owners);
        assert_eq!(
            &*order.borrow(),
            &["graphics", "surface", "provider", "engine"]
        );
    }

    #[test]
    fn early_return_cleanup_drops_each_owner_once_after_surface_transfer() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let mut owners = RuntimeOwners::<DropProbe, DropProbe, DropProbe, ()>::new();
        owners
            .attach_engine(DropProbe {
                name: "engine",
                order: Rc::clone(&order),
            })
            .unwrap();
        owners
            .attach_provider(DropProbe {
                name: "provider",
                order: Rc::clone(&order),
            })
            .unwrap();
        owners
            .attach_surface(DropProbe {
                name: "surface",
                order: Rc::clone(&order),
            })
            .unwrap();

        // Model an early return after the surface has been detached for its
        // native destroy operation. The remaining RuntimeOwners value still
        // owns provider and engine and must release each exactly once.
        drop(owners.take_surface());
        drop(owners);

        assert_eq!(&*order.borrow(), &["surface", "provider", "engine"]);
    }
}
