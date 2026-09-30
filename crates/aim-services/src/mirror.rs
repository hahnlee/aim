//! State of the original services that a native service decides by, kept
//! in the service host and updated by the owners' own change
//! notifications, so that a decision needs no call into system_server
//! (#432).
//!
//! The rule that keeps a mirrored decision the owner's:
//!
//! - A value is kept only while the owner's change notification is
//!   registered. The listener is registered before the first query, and a
//!   query stores its answer only if no notification arrived since it
//!   began (a generation count), so a kept value is never older than the
//!   last notification.
//! - A notification drops what it may have changed; the next decision asks
//!   the owner again, once.
//! - When the owner dies (system_server restarts), the registration dies
//!   with it: the mirror is dropped and decisions are synchronous queries
//!   until the listener is registered again.
//!
//! The owners send their notifications, one-way, when they commit a
//! change. A decision made while a change's notification is still in
//! flight is the decision from just before the change, as it is for the
//! original's other clients of the same listeners.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;

use aim_binder_host::parcel::Exception;

type Result<T> = std::result::Result<T, Exception>;

struct State<K, V> {
    watched: bool,
    generation: u64,
    values: HashMap<K, V>,
}

pub struct Mirror<K, V> {
    state: Mutex<State<K, V>>,
    /// Held while the listener is registered, so it is registered once.
    watching: Mutex<()>,
}

impl<K: Eq + Hash, V: Clone> Mirror<K, V> {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                watched: false,
                generation: 0,
                values: HashMap::new(),
            }),
            watching: Mutex::new(()),
        }
    }

    /// `key`'s value: the kept one, else `fetch`'s, kept if `watch` has
    /// registered the owner's listener and nothing changed meanwhile.
    pub fn get(
        &self,
        key: K,
        watch: impl FnOnce() -> Result<()>,
        fetch: impl FnOnce() -> Result<V>,
    ) -> Result<V> {
        if let Some(value) = self.state.lock().unwrap().values.get(&key) {
            return Ok(value.clone());
        }
        let generation = {
            let _watching = self.watching.lock().unwrap();
            if !self.state.lock().unwrap().watched {
                // Without the listener the query is made every time.
                if let Err(e) = watch() {
                    eprintln!("services: cannot watch for changes: {}", e.message);
                    return fetch();
                }
                self.state.lock().unwrap().watched = true;
            }
            self.state.lock().unwrap().generation
        };
        let value = fetch()?;
        let mut st = self.state.lock().unwrap();
        if st.watched && st.generation == generation {
            st.values.insert(key, value.clone());
        }
        Ok(value)
    }

    /// The owner told of a change.
    pub fn invalidate(&self) {
        let mut st = self.state.lock().unwrap();
        st.generation += 1;
        st.values.clear();
    }

    /// The owner, and with it the listener's registration, died.
    pub fn unwatch(&self) {
        let mut st = self.state.lock().unwrap();
        st.watched = false;
        st.generation += 1;
        st.values.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn keeps_values_while_watched() {
        let mirror = Mirror::<i32, i32>::new();
        let fetches = Cell::new(0);
        let fetch = || {
            fetches.set(fetches.get() + 1);
            Ok(fetches.get())
        };
        assert_eq!(mirror.get(1, || Ok(()), fetch).unwrap(), 1);
        assert_eq!(mirror.get(1, || panic!("watched twice"), fetch).unwrap(), 1);
        mirror.invalidate();
        assert_eq!(mirror.get(1, || panic!("watched twice"), fetch).unwrap(), 2);
        mirror.unwatch();
        assert_eq!(mirror.get(1, || Ok(()), fetch).unwrap(), 3);
        assert_eq!(fetches.get(), 3);
    }

    #[test]
    fn unwatched_values_are_not_kept() {
        let mirror = Mirror::<i32, i32>::new();
        let fail = || {
            Err(Exception::new(
                aim_binder_host::parcel::EX_ILLEGAL_STATE,
                "down",
            ))
        };
        assert_eq!(mirror.get(1, fail, || Ok(1)).unwrap(), 1);
        assert_eq!(mirror.get(1, fail, || Ok(2)).unwrap(), 2);
    }

    #[test]
    fn a_change_during_a_query_is_not_lost() {
        let mirror = Mirror::<i32, i32>::new();
        let value = mirror
            .get(
                1,
                || Ok(()),
                || {
                    mirror.invalidate();
                    Ok(1)
                },
            )
            .unwrap();
        assert_eq!(value, 1);
        assert_eq!(mirror.get(1, || Ok(()), || Ok(2)).unwrap(), 2);
    }
}
