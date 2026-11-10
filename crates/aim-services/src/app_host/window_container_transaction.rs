//! WindowContainerTransaction at android-16.0.0_r1: Parcel.writeMap uses
//! VAL_IBINDER keys and length-prefixed VAL_PARCELABLE Change values.
//! Copyright (C) The Android Open Source Project, Apache License 2.0.
use super::config::{CONFIG_WINDOW_CONFIGURATION, Configuration, Rect};
use aim_binder_host::parcel::{Binder, EX_UNSUPPORTED_OPERATION, Exception, Reader};
use aim_service_aidl::ReadParcelable;

#[derive(Debug, PartialEq)]
pub(super) struct Change {
    pub token: Binder,
    /// `WINDOW_CONFIG_BOUNDS`: `Some(None)` is an empty rectangle, which
    /// inherits the parent's bounds (`setBounds(null)`).
    pub bounds: Option<Option<Rect>>,
    /// `Change.mWindowingMode`, unless negative (unchanged).
    pub mode: Option<i32>,
}
/// A `HierarchyOp` of a task hierarchy.
#[derive(Debug, PartialEq)]
pub(super) enum Hop {
    /// `reorder`: the task over or under its siblings.
    Reorder(Binder, bool),
    /// `reparent`: the task into a new parent, the display area if `None`,
    /// over or under its tasks.
    Reparent {
        task: Binder,
        parent: Option<Binder>,
        to_top: bool,
    },
    /// `reparentTasks`: the tasks of a parent of the given windowing modes
    /// and activity types into another, either the display area if `None`.
    ReparentChildren {
        from: Option<Binder>,
        to: Option<Binder>,
        modes: Option<Vec<i32>>,
        types: Option<Vec<i32>>,
        to_top: bool,
        top_only: bool,
    },
    /// `setAdjacentRootSet`: root tasks adjacent to each other.
    SetAdjacentRoots(Vec<Binder>),
    /// `clearAdjacentRoots`: the root task leaves its adjacent set.
    ClearAdjacentRoots(Binder),
    /// `setLaunchAdjacentFlagRoot`, or `clearLaunchAdjacentFlagRoot` if
    /// `clear`.
    LaunchAdjacentFlagRoot { root: Binder, clear: bool },
}
#[derive(Debug)]
pub(super) struct Transaction {
    pub changes: Vec<Change>,
    pub hierarchy: Vec<Hop>,
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
fn optional_binder(r: &mut Reader<'_>) -> Result<Option<Binder>, Exception> {
    r.read_binder().map_err(|_| malformed())
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
/// `Parcel.writeIntArray` or `writeBinderArray` of `item`s: -1 for null.
fn array<T>(
    r: &mut Reader<'_>,
    item: fn(&mut Reader<'_>) -> Result<T, Exception>,
) -> Result<Option<Vec<T>>, Exception> {
    let n = int(r)?;
    if n == -1 {
        return Ok(None);
    }
    let n = usize::try_from(n).map_err(|_| malformed())?;
    if n > r.remaining() / 4 {
        return Err(malformed());
    }
    (0..n).map(|_| item(r)).collect::<Result<_, _>>().map(Some)
}
impl Transaction {
    /// The enclosing AIDL typed object has already been unwrapped.
    pub fn read(r: &mut Reader<'_>) -> Result<Self, Exception> {
        let mut out = Self {
            changes: Vec::new(),
            hierarchy: Vec::new(),
        };
        let mut keys = Vec::new();
        for _ in 0..count(r)? {
            expect(r, 15)?; // VAL_IBINDER
            let token = binder(r)?;
            if keys.contains(&token) {
                return Err(malformed());
            }
            keys.push(token);
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
            let masks = (int(r)?, int(r)?);
            if boolean(r)? {
                return Err(unsupported());
            }
            let mode = match int(r)? {
                // Pinned needs the PiP owner (#2616).
                2 => return Err(unsupported()),
                // `applyChanges` ignores any mode below 0.
                m => (m >= 0).then_some(m),
            };
            expect(r, -1)?; // unchanged activity windowing mode
            if r.position() != end {
                return Err(malformed());
            }
            let bounds = match masks {
                (0, 0) => None,
                (CONFIG_WINDOW_CONFIGURATION, 1) => {
                    // WINDOW_CONFIG_BOUNDS only
                    let b = c.window.bounds;
                    let width = b.right.checked_sub(b.left).ok_or_else(malformed)?;
                    let height = b.bottom.checked_sub(b.top).ok_or_else(malformed)?;
                    Some((width > 0 && height > 0).then_some(b))
                }
                _ => return Err(unsupported()),
            };
            // An empty Change applies nothing (`applyChanges`).
            if bounds.is_some() || mode.is_some() {
                out.changes.push(Change {
                    token,
                    bounds,
                    mode,
                });
            }
        }
        for _ in 0..count(r)? {
            expect(r, 1)?; // typed HierarchyOp present
            // HIERARCHY_OP_TYPE_REPARENT, _REORDER, _CHILDREN_TASKS_REPARENT,
            // _SET_ADJACENT_ROOTS, _SET_LAUNCH_ADJACENT_FLAG_ROOT or
            // _CLEAR_ADJACENT_ROOTS
            let kind = int(r)?;
            if !matches!(kind, 0..=2 | 4 | 6 | 15) {
                return Err(unsupported());
            }
            let container = optional_binder(r)?;
            let containers = array(r, binder)?;
            expect(r, 0)?; // bounds
            if boolean(r)? {
                return Err(unsupported());
            } // includingParents
            let reparent = optional_binder(r)?;
            expect(r, 0)?; // InsetsFrameProvider
            null_binder(r)?;
            let to_top = boolean(r)?;
            let top_only = boolean(r)?;
            let modes = array(r, int)?;
            let types = array(r, int)?;
            for _ in 0..2 {
                expect(r, -1)?;
            } // two Bundles
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
            // The adjacency ops' builders set their tokens and clearRoot only.
            if matches!(kind, 4 | 6 | 15)
                && (reparent.is_some()
                    || top_only
                    || modes.is_some()
                    || types.is_some()
                    || (to_top && kind != 6))
            {
                return Err(unsupported());
            }
            match (kind, containers) {
                // setAdjacentRootsHierarchyOp reads the containers alone.
                (4, Some(roots)) if container.is_none() => {
                    out.hierarchy.push(Hop::SetAdjacentRoots(roots));
                    continue;
                }
                (4, None) => return Err(malformed()),
                (6, None) => {
                    out.hierarchy.push(Hop::LaunchAdjacentFlagRoot {
                        root: container.ok_or_else(malformed)?,
                        clear: to_top,
                    });
                    continue;
                }
                (15, None) => {
                    out.hierarchy
                        .push(Hop::ClearAdjacentRoots(container.ok_or_else(malformed)?));
                    continue;
                }
                (0..=2, None) => {}
                _ => return Err(unsupported()),
            }
            if kind == 2 {
                out.hierarchy.push(Hop::ReparentChildren {
                    from: container,
                    to: reparent,
                    modes,
                    types,
                    to_top,
                    top_only,
                });
                continue;
            }
            // reparentTopOnly and the filters are reparentTasks'.
            if top_only || modes.is_some() || types.is_some() {
                return Err(unsupported());
            }
            let task = container.ok_or_else(malformed)?;
            out.hierarchy.push(if kind == 1 {
                // createForReorder
                if reparent != Some(task) {
                    return Err(malformed());
                }
                Hop::Reorder(task, to_top)
            } else {
                Hop::Reparent {
                    task,
                    parent: reparent,
                    to_top,
                }
            });
        }
        null_binder(r)?; // error callback token
        null_binder(r)?; // task fragment organizer
        if r.remaining() != 0 {
            return Err(malformed());
        }
        if out.changes.is_empty() && out.hierarchy.is_empty() {
            return Err(unsupported());
        }
        Ok(out)
    }
}

/// An admitted hierarchy operation, of task ids.
enum Admitted {
    Reorder(i32, bool),
    Reparent(i32, Option<i32>, bool),
    ReparentChildren(Children),
}
/// An admitted `reparentTasks`.
struct Children {
    from: Option<i32>,
    to: Option<i32>,
    modes: Option<Vec<i32>>,
    types: Option<Vec<i32>>,
    to_top: bool,
    top_only: bool,
}

impl super::AppHost {
    pub(super) fn apply_window_transaction(
        self: &std::sync::Arc<Self>,
        t: Transaction,
    ) -> Result<(), Exception> {
        // Resolve every token and validate every operation before invoking a mutating owner.
        // The task of a token, unless it is unknown or removed (detached).
        let attached = |token| {
            let id = self.task_of_token(token)?;
            let attached = self.tasks.bounds(id).is_some()
                && (self.tasks.root(id).is_some()
                    || self.activities.stack.lock().unwrap().task(id).is_some());
            attached.then_some(id)
        };
        let resolve = |token| {
            attached(token)
                .ok_or_else(|| Exception::illegal_argument("unknown or removed WCT task"))
        };
        // A parent: an organizer root task, else the display area. Nesting a
        // task in an app task is not served (#2616).
        let parent = |token: Option<Binder>| {
            token
                .map(|token| {
                    let id = resolve(token)?;
                    match self.tasks.root(id) {
                        Some(_) => Ok(id),
                        None => Err(unsupported()),
                    }
                })
                .transpose()
        };
        let mut roots = Vec::new();
        let mut changes = Vec::new();
        for c in t.changes {
            let id = resolve(c.token)?;
            if self.tasks.root(id).is_some() {
                // Root tasks of another windowing mode (#2616).
                if c.mode.is_some_and(|m| m != 6) {
                    return Err(unsupported());
                }
                if let Some(bounds) = c.bounds {
                    roots.push((id, bounds));
                }
                continue;
            }
            self.task_change(id, c.mode, c.bounds)
                .map_err(Exception::illegal_argument)?;
            changes.push((id, c.mode, c.bounds));
        }
        // The adjacency the hops leave, each admitted in the adjacency the
        // ones before it left (`setLaunchAdjacentFlagRoot` after
        // `setAdjacentRoots`). It stays locked until it is committed, so a
        // root task removed meanwhile leaves it after the commit.
        let mut committed = self.tasks.adjacency.lock().unwrap();
        let mut adjacency = committed.clone();
        // An adjacency op on an unknown or detached container is skipped, the
        // rest of the transaction applied.
        let detached = |op| eprintln!("app host: {op}: unknown or detached container");
        let mut adjacency_ops = false;
        let created = |id, op| match self.tasks.root(id) {
            Some(_) => Ok(id),
            None => Err(Exception::illegal_argument(format!(
                "{op}: Not created by organizer root={id}"
            ))),
        };
        let mut hierarchy = Vec::new();
        for hop in t.hierarchy {
            adjacency_ops |= matches!(
                hop,
                Hop::SetAdjacentRoots(_)
                    | Hop::ClearAdjacentRoots(_)
                    | Hop::LaunchAdjacentFlagRoot { .. }
            );
            hierarchy.push(match hop {
                Hop::SetAdjacentRoots(tokens) => {
                    let op = "setAdjacentRootsHierarchyOp";
                    let (mut ids, mut skipped) = (Vec::new(), false);
                    for token in tokens {
                        let Some(id) = attached(token) else {
                            detached(op);
                            skipped = true;
                            break;
                        };
                        let id = created(id, op)?;
                        if ids.contains(&id) {
                            return Err(Exception::illegal_argument(format!(
                                "setAdjacentRootsHierarchyOp: called with same root twice={id}"
                            )));
                        }
                        ids.push(id);
                    }
                    if skipped {
                        continue;
                    }
                    if ids.len() < 2 {
                        return Err(Exception::illegal_argument(
                            "Adjacent TaskFragments must contain at least two TaskFragments",
                        ));
                    }
                    adjacency.join(&ids);
                    continue;
                }
                Hop::ClearAdjacentRoots(token) => {
                    let op = "clearAdjacentRootsHierarchyOp";
                    match attached(token) {
                        Some(id) => adjacency.leave(created(id, op)?),
                        None => detached(op),
                    }
                    continue;
                }
                Hop::LaunchAdjacentFlagRoot { root, clear } => {
                    let Some(id) = attached(root) else {
                        detached("setLaunchAdjacentFlagRoot");
                        continue;
                    };
                    if self.tasks.root(id).is_none() {
                        return Err(Exception::new(
                            EX_UNSUPPORTED_OPERATION,
                            format!("Cannot set non-organized task as adjacent flag root: {id}"),
                        ));
                    }
                    if !clear && adjacency.of(id).is_empty() {
                        return Err(Exception::new(
                            EX_UNSUPPORTED_OPERATION,
                            format!("Cannot set non-adjacent task as adjacent flag root: {id}"),
                        ));
                    }
                    adjacency.launch_root = (!clear).then_some(id);
                    continue;
                }
                Hop::Reorder(token, top) => Admitted::Reorder(resolve(token)?, top),
                Hop::Reparent {
                    task,
                    parent: to,
                    to_top,
                } => {
                    let id = resolve(task)?;
                    // Nested organizer root tasks (#2616).
                    if self.tasks.root(id).is_some() {
                        return Err(unsupported());
                    }
                    Admitted::Reparent(id, parent(to)?, to_top)
                }
                Hop::ReparentChildren { from, to, .. } if from.is_none() && to.is_none() => {
                    return Err(Exception::illegal_argument(
                        "reparentChildrenTasksHierarchyOp: no parents",
                    ));
                }
                Hop::ReparentChildren {
                    from,
                    to,
                    modes,
                    types,
                    to_top,
                    top_only,
                } => Admitted::ReparentChildren(Children {
                    from: parent(from)?,
                    to: parent(to)?,
                    modes,
                    types,
                    to_top,
                    top_only,
                }),
            });
        }
        if changes.is_empty() && roots.is_empty() && hierarchy.is_empty() && !adjacency_ops {
            return Err(unsupported());
        }
        *committed = adjacency;
        drop(committed);
        for (id, bounds) in roots {
            self.set_root_bounds(id, bounds).map_err(owner_error)?;
        }
        for (id, mode, bounds) in changes {
            let change = self.task_change(id, mode, bounds).map_err(owner_error)?;
            self.apply_task_change(id, change).map_err(owner_error)?;
        }
        for hop in hierarchy {
            match hop {
                Admitted::Reorder(id, top) => self.reorder(id, top),
                // sanitizeAndApplyHierarchyOpForTask: a task already there
                // moves with its root task.
                Admitted::Reparent(id, to, top) if self.tasks.parent(id) == to => {
                    self.reorder(to.unwrap_or(id), top)
                }
                Admitted::Reparent(id, to, top) => {
                    if to.is_none() || self.organizer_task_info(id)?.supports_multi_window {
                        self.reparent_task(id, to, top)
                    } else {
                        eprintln!("app host: task {id} doesn't support multi-window mode");
                        Ok(())
                    }
                }
                Admitted::ReparentChildren(c) => self.reparent_children(c),
            }
            .map_err(owner_error)?;
        }
        self.organizer_tasks_changed()
    }

    fn reorder(self: &std::sync::Arc<Self>, id: i32, top: bool) -> Result<(), String> {
        if self.tasks.root(id).is_some() {
            self.reorder_root_task(id, top)
        } else if top {
            self.focus_task(id)
        } else {
            self.reorder_task_to_back(id)
        }
    }

    /// `reparentChildrenTasksHierarchyOp` from the organizer root task
    /// `from` to `to`, either the display area if `None`: the tasks it did
    /// not create of the modes and types, in their order, the top one only
    /// if `top_only`.
    fn reparent_children(self: &std::sync::Arc<Self>, c: Children) -> Result<(), String> {
        let Children {
            from,
            to,
            modes,
            types,
            to_top,
            top_only,
        } = c;
        if from == to {
            eprintln!("app host: reparentTasks parent not changing");
            return Ok(());
        }
        let mut tasks = Vec::new();
        for id in self.tasks.children(from) {
            if self.tasks.root(id).is_some() {
                continue;
            }
            let info = self.organizer_task_info(id).map_err(|e| e.message)?;
            let mode = self.tasks.windowing_mode(id).ok_or("task gone")?;
            if (to.is_some() && !info.supports_multi_window)
                || types
                    .as_ref()
                    .is_some_and(|t| !t.is_empty() && !t.contains(&info.activity_type))
                || modes
                    .as_ref()
                    .is_some_and(|m| !m.is_empty() && !m.contains(&mode))
            {
                continue;
            }
            if to_top {
                tasks.insert(0, id);
            } else {
                tasks.push(id);
            }
            if top_only {
                break;
            }
        }
        for id in tasks {
            self.reparent_task(id, to, to_top)?;
        }
        Ok(())
    }
}
fn owner_error(e: String) -> Exception {
    Exception::new(aim_binder_host::parcel::EX_ILLEGAL_STATE, e)
}

#[cfg(test)]
#[path = "window_container_transaction_tests.rs"]
pub(super) mod tests;
