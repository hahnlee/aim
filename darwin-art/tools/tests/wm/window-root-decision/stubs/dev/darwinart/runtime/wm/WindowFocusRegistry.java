package dev.darwinart.runtime.wm;

public final class WindowFocusRegistry {
    static final class WindowSpec {
        final Object rootToken;
        WindowSpec(Object root) { rootToken = root; }
    }

    static final class DecisionSnapshot {
        final WindowSpec window;
        final long epoch;
        DecisionSnapshot(WindowSpec selected, long transitionEpoch) {
            window = selected;
            epoch = transitionEpoch;
        }
    }

    private DecisionSnapshot selected = new DecisionSnapshot(null, 0L);

    DecisionSnapshot decisionSnapshot(int displayId) { return selected; }
    void select(Object root, long epoch) {
        selected = new DecisionSnapshot(root == null ? null : new WindowSpec(root), epoch);
    }
}
