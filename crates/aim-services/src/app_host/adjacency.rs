//! Organizer root tasks adjacent to each other (split screen), as
//! `TaskFragment.AdjacentSet` and `TaskDisplayArea.mLaunchAdjacentFlagRootTask`
//! keep them at android-16.0.0_r1, and the display area's launch root tasks
//! (`mLaunchRootTasks`). In window mode each of their tasks keeps
//! its own Mac window, at its root task's bounds, so adjacent roots' windows
//! are tiled side by side (docs/windows.md).
//! Copyright (C) The Android Open Source Project, Apache License 2.0.

use super::config::Rect;

/// The display area's adjacent root tasks.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Adjacency {
    /// The sets of root tasks adjacent to each other, each of two or more.
    sets: Vec<Vec<i32>>,
    /// `mLaunchAdjacentFlagRootTask`.
    pub launch_root: Option<i32>,
    /// `mLaunchRootTasks`, the latest last.
    launch_roots: Vec<LaunchRoot>,
}

/// `LaunchRootTaskDef`: a root task new tasks of its windowing modes and
/// activity types go into.
#[derive(Clone, Debug, PartialEq)]
struct LaunchRoot {
    task: i32,
    modes: Vec<i32>,
    types: Vec<i32>,
}

impl Adjacency {
    /// The root tasks adjacent to root task `id`.
    pub fn of(&self, id: i32) -> Vec<i32> {
        self.sets
            .iter()
            .find(|s| s.contains(&id))
            .map(|s| s.iter().copied().filter(|&r| r != id).collect())
            .unwrap_or_default()
    }

    /// `AdjacentSet.setAsAdjacent`: each of `ids` leaves its set, and they
    /// become one.
    pub fn join(&mut self, ids: &[i32]) {
        for &id in ids {
            self.leave(id);
        }
        self.sets.push(ids.to_vec());
    }

    /// `removeFromAdjacentTaskFragments`: a set left with fewer than two
    /// dissolves (`AdjacentSet.remove`).
    pub fn leave(&mut self, id: i32) {
        for set in &mut self.sets {
            set.retain(|&r| r != id);
        }
        self.sets.retain(|s| s.len() >= 2);
    }

    /// `TaskDisplayArea.setLaunchRootTask`: root task `id` becomes the
    /// latest launch root task of `modes` and `types`, or none of either.
    pub fn set_launch_root(&mut self, id: i32, modes: Vec<i32>, types: Vec<i32>) {
        self.launch_roots.retain(|r| r.task != id);
        if !modes.is_empty() || !types.is_empty() {
            self.launch_roots.push(LaunchRoot {
                task: id,
                modes,
                types,
            });
        }
    }

    /// The latest launch root task of windowing `mode` and activity type
    /// `kind` (`LaunchRootTaskDef.contains`).
    pub fn launch_root_of(&self, mode: i32, kind: i32) -> Option<i32> {
        self.launch_roots
            .iter()
            .rev()
            .find(|r| r.modes.contains(&mode) && r.types.contains(&kind))
            .map(|r| r.task)
    }

    /// Root task `id` is gone (`TaskDisplayArea.onRootTaskRemoved`; with
    /// its organizer, `removeLaunchRootTask`).
    pub fn remove(&mut self, id: i32) {
        self.leave(id);
        if self.launch_root == Some(id) {
            self.launch_root = None;
        }
        self.launch_roots.retain(|r| r.task != id);
    }
}

/// The bounds `b` of a root task tiled with one whose bounds `old` become
/// `new`. Moved at the same size, the tiles move together. Resized, as the
/// split divider keeps them: each of its edges that met an opposite edge of
/// `old` follows it, and the edges along a shared one stay aligned with
/// `new`'s.
pub(super) fn tiled(mut b: Rect, old: Rect, new: Rect) -> Rect {
    if new.width() == old.width() && new.height() == old.height() {
        b.offset(new.left - old.left, new.top - old.top);
        return b;
    }
    let beside = b.left == old.right || b.right == old.left;
    let stacked = b.top == old.bottom || b.bottom == old.top;
    if b.left == old.right {
        b.left = new.right;
    }
    if b.right == old.left {
        b.right = new.left;
    }
    if b.top == old.bottom {
        b.top = new.bottom;
    }
    if b.bottom == old.top {
        b.bottom = new.top;
    }
    if beside && b.top == old.top {
        b.top = new.top;
    }
    if beside && b.bottom == old.bottom {
        b.bottom = new.bottom;
    }
    if stacked && b.left == old.left {
        b.left = new.left;
    }
    if stacked && b.right == old.right {
        b.right = new.right;
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_join_leave_dissolve_and_go_with_their_roots() {
        let mut a = Adjacency::default();
        a.join(&[1, 2]);
        assert_eq!((a.of(1), a.of(2), a.of(3)), (vec![2], vec![1], vec![]));
        // A root joining another set leaves its own, which dissolves.
        a.join(&[2, 3, 4]);
        assert_eq!((a.of(1), a.of(2)), (vec![], vec![3, 4]));
        // Leaving a set of three keeps the other two adjacent.
        a.leave(3);
        assert_eq!((a.of(2), a.of(3)), (vec![4], vec![]));
        a.launch_root = Some(4);
        a.remove(2);
        assert_eq!((a.of(4), a.launch_root), (vec![], Some(4)));
        a.join(&[4, 5]);
        a.remove(4);
        assert_eq!(a, Adjacency::default());
    }

    #[test]
    fn tiled_edges_follow_the_resized_root() {
        let r = |left, top, right, bottom| Rect {
            left,
            top,
            right,
            bottom,
        };
        let (left, right) = (r(0, 0, 540, 1920), r(540, 0, 1080, 1920));
        // The divider moves; the window below the menu bar takes the other
        // with it.
        assert_eq!(
            tiled(right, left, r(0, 70, 600, 1920)),
            r(600, 70, 1080, 1920)
        );
        assert_eq!(
            tiled(left, right, r(500, 0, 1080, 1900)),
            r(0, 0, 500, 1900)
        );
        let (top, bottom) = (r(0, 0, 1080, 960), r(0, 960, 1080, 1920));
        assert_eq!(
            tiled(bottom, top, r(10, 0, 1070, 1000)),
            r(10, 1000, 1070, 1920)
        );
        // Roots that do not meet stay where they are.
        assert_eq!(tiled(right, r(0, 0, 500, 1920), left), right);
        // A tile dragged by its title bar takes the others with it.
        assert_eq!(
            tiled(right, left, r(200, 30, 740, 1950)),
            r(740, 30, 1280, 1950)
        );
        assert_eq!(
            tiled(left, right, r(340, 0, 880, 1920)),
            r(-200, 0, 340, 1920)
        );
    }
}
