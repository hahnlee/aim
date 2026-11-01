//! WindowContainerTransaction at android-16.0.0_r1: Parcel.writeMap uses
//! VAL_IBINDER keys and length-prefixed VAL_PARCELABLE Change values.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::config::{CONFIG_WINDOW_CONFIGURATION, Configuration, Rect};
use aim_binder_host::parcel::{Binder, EX_UNSUPPORTED_OPERATION, Exception, Reader};
use aim_service_aidl::ReadParcelable;

#[derive(Debug)]
pub(super) struct Transaction {
    pub bounds: Vec<(Binder, Rect)>,
    pub reorders: Vec<(Binder, bool)>,
}
fn malformed() -> Exception {
    Exception::illegal_argument("malformed WindowContainerTransaction")
}
fn unsupported() -> Exception {
    Exception::new(
        EX_UNSUPPORTED_OPERATION,
        "unsupported WindowContainerTransaction (#2616)",
    )
}
fn int(r: &mut Reader<'_>) -> Result<i32, Exception> {
    r.read_i32().map_err(|_| malformed())
}
fn boolean(r: &mut Reader<'_>) -> Result<bool, Exception> {
    match int(r)? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(malformed()),
    }
}
fn binder(r: &mut Reader<'_>) -> Result<Binder, Exception> {
    r.read_binder()
        .map_err(|_| malformed())?
        .ok_or_else(malformed)
}
fn null_binder(r: &mut Reader<'_>) -> Result<(), Exception> {
    if r.read_binder().map_err(|_| malformed())?.is_some() {
        return Err(unsupported());
    }
    Ok(())
}
fn expect(r: &mut Reader<'_>, value: i32) -> Result<(), Exception> {
    if int(r)? != value {
        return Err(unsupported());
    }
    Ok(())
}
fn count(r: &mut Reader<'_>) -> Result<usize, Exception> {
    let n = usize::try_from(int(r)?).map_err(|_| malformed())?;
    if n > r.remaining() / 4 {
        return Err(malformed());
    }
    Ok(n)
}
impl Transaction {
    /// The enclosing AIDL typed object has already been unwrapped.
    pub fn read(r: &mut Reader<'_>) -> Result<Self, Exception> {
        let mut out = Self {
            bounds: Vec::new(),
            reorders: Vec::new(),
        };
        for _ in 0..count(r)? {
            expect(r, 15)?; // VAL_IBINDER
            let token = binder(r)?;
            if out.bounds.iter().any(|(b, _)| *b == token) {
                return Err(malformed());
            }
            expect(r, 4)?; // VAL_PARCELABLE
            let len = usize::try_from(int(r)?).map_err(|_| malformed())?;
            let end = r.position().checked_add(len).ok_or_else(malformed)?;
            if len > r.remaining() || len % 4 != 0 {
                return Err(malformed());
            }
            if r.read_string16().map_err(|_| malformed())?.as_deref()
                != Some("android.window.WindowContainerTransaction$Change")
            {
                return Err(unsupported());
            }
            let c = Configuration::read_from(r).map_err(|_| malformed())?;
            for default in [true, false, false, false, false] {
                if boolean(r)? != default {
                    return Err(unsupported());
                }
            }
            // No conditional Change payloads (PiP, surface transaction, relative bounds).
            expect(r, 0)?;
            expect(r, CONFIG_WINDOW_CONFIGURATION)?;
            expect(r, 1)?; // WINDOW_CONFIG_BOUNDS only
            if boolean(r)? {
                return Err(unsupported());
            }
            expect(r, -1)?; // unchanged task/activity windowing modes
            expect(r, -1)?;
            if r.position() != end {
                return Err(malformed());
            }
            let b = c.window.bounds;
            // Clearing/inheriting bounds remains outside this slice (#2616).
            let width = b.right.checked_sub(b.left).ok_or_else(malformed)?;
            let height = b.bottom.checked_sub(b.top).ok_or_else(malformed)?;
            if width <= 0 || height <= 0 {
                return Err(unsupported());
            }
            out.bounds.push((token, b));
        }
        for _ in 0..count(r)? {
            expect(r, 1)?; // typed HierarchyOp present
            expect(r, 1)?; // HIERARCHY_OP_TYPE_REORDER
            let token = binder(r)?;
            expect(r, -1)?; // containers array
            expect(r, 0)?; // bounds
            if boolean(r)? {
                return Err(unsupported());
            } // includingParents
            if binder(r)? != token {
                return Err(malformed());
            } // createForReorder
            expect(r, 0)?; // InsetsFrameProvider
            null_binder(r)?;
            let to_top = boolean(r)?;
            if boolean(r)? {
                return Err(unsupported());
            } // reparentTopOnly
            for _ in 0..4 {
                expect(r, -1)?;
            } // mode/type arrays, two Bundles
            for _ in 0..5 {
                expect(r, 0)?;
            } // Intent, fragment, keyguard, pending, shortcut
            for _ in 0..3 {
                if boolean(r)? {
                    return Err(unsupported());
                }
            }
            expect(r, 0)?; // excludeInsetsTypes
            if boolean(r)? {
                return Err(unsupported());
            } // launchAdjacentDisabled
            expect(r, 0)?; // safeRegionBounds
            out.reorders.push((token, to_top));
        }
        null_binder(r)?; // error callback token
        null_binder(r)?; // task fragment organizer
        if r.remaining() != 0 {
            return Err(malformed());
        }
        if out.bounds.is_empty() && out.reorders.is_empty() {
            return Err(unsupported());
        }
        Ok(out)
    }
}

impl super::AppHost {
    pub(super) fn apply_window_transaction(
        self: &std::sync::Arc<Self>,
        t: Transaction,
    ) -> Result<(), Exception> {
        // Resolve every token and validate every operation before invoking a mutating owner.
        let resolve = |token| {
            let id = self
                .activities
                .task_tokens
                .lock()
                .unwrap()
                .iter()
                .find_map(|(id, binder)| (*binder == token).then_some(*id))
                .ok_or_else(|| Exception::illegal_argument("unknown WCT task token"))?;
            if self.tasks.bounds(id).is_none()
                || self.activities.stack.lock().unwrap().task(id).is_none()
            {
                return Err(Exception::illegal_argument("removed WCT task"));
            }
            Ok(id)
        };
        let mut bounds = Vec::new();
        for (token, b) in t.bounds {
            let id = resolve(token)?;
            bounds.push((
                id,
                self.task_move_bounds(id, b)
                    .map_err(Exception::illegal_argument)?,
            ));
        }
        let reorders = t
            .reorders
            .into_iter()
            .map(|(token, top)| Ok((resolve(token)?, top)))
            .collect::<Result<Vec<_>, Exception>>()?;
        if bounds.is_empty() && reorders.is_empty() {
            return Err(unsupported());
        }
        for (id, b) in bounds {
            self.move_task(id, b).map_err(owner_error)?;
        }
        for (id, top) in reorders {
            if top {
                self.focus_task(id)
            } else {
                self.reorder_task_to_back(id)
            }
            .map_err(owner_error)?;
        }
        self.organizer_tasks_changed()
    }
}
fn owner_error(e: String) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, e)
}

#[cfg(test)]
#[path = "window_container_transaction_tests.rs"]
pub(super) mod tests;
