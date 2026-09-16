//! Host blocking mechanism for property-area change waits; no polling.
use super::*;
use std::time::{Duration, Instant};

impl PropertyArea {
    pub fn resume_waiters(&self) -> Result<(), &'static str> {
        self.changes
            .lock()
            .map_err(|_| "property wait lock poisoned")?
            .1 = false;
        Ok(())
    }
    pub fn close_waiters(&self) {
        let mut state = self
            .changes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.0 = state.0.wrapping_add(1);
        state.1 = true;
        self.changed.notify_all();
    }
    pub fn wait(
        &self,
        token: *const c_void,
        old: u32,
        timeout: Option<Duration>,
    ) -> Result<Option<u32>, &'static str> {
        let deadline = timeout
            .map(|duration| {
                Instant::now()
                    .checked_add(duration)
                    .ok_or("invalid property timeout")
            })
            .transpose()?;
        let mut state = self
            .changes
            .lock()
            .map_err(|_| "property wait lock poisoned")?;
        let epoch = state.0;
        loop {
            if state.1 || state.0 != epoch {
                return Err("property area closed");
            }
            let current = if token.is_null() {
                self.area_serial()
            } else {
                self.serial(token)?.ok_or("invalid property token")?
            };
            if current != old {
                return Ok(Some(current));
            }
            state = if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Ok(None);
                }
                self.changed
                    .wait_timeout(state, remaining)
                    .map_err(|_| "property wait lock poisoned")?
                    .0
            } else {
                self.changed
                    .wait(state)
                    .map_err(|_| "property wait lock poisoned")?
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waits_for_selected_property_not_unrelated_updates() {
        let area = Arc::new(PropertyArea::new(vec![(b"key".to_vec(), b"a".to_vec())]).unwrap());
        let token = area.find(b"key").unwrap() as usize;
        let old = area.serial(token as *const _).unwrap().unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_area = area.clone();
        let worker = std::thread::spawn(move || {
            tx.send(worker_area.wait(token as *const _, old, Some(Duration::from_secs(2))))
                .unwrap();
        });
        area.update(b"other", b"x").unwrap();
        assert!(rx.recv_timeout(Duration::from_millis(10)).is_err());
        area.update(b"key", b"b").unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(),
            area.serial(token as *const _).unwrap()
        );
        worker.join().unwrap();
    }
    #[test]
    fn global_timeout_and_shutdown_are_distinct() {
        let area = Arc::new(PropertyArea::new(vec![]).unwrap());
        assert_eq!(
            area.wait(std::ptr::null(), 0, Some(Duration::ZERO))
                .unwrap(),
            None
        );
        area.update(b"new", b"value").unwrap();
        assert_eq!(area.wait(std::ptr::null(), 0, None).unwrap(), Some(1));
        let worker_area = area.clone();
        let worker = std::thread::spawn(move || worker_area.wait(std::ptr::null(), 1, None));
        area.close_waiters();
        assert_eq!(worker.join().unwrap(), Err("property area closed"));
        area.resume_waiters().unwrap();
        assert_eq!(
            area.wait(std::ptr::null(), 1, Some(Duration::ZERO))
                .unwrap(),
            None
        );
    }
}
