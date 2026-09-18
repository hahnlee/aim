package dev.darwinart.runtime.wm;

/** WMS root activation policy. All calls hold the shared publication controller lock. */
final class WindowRootActivationPolicy {
    private final WindowFocusRegistry registry;
    private DesktopRootRegistry.Registration activeRoot;
    private DesktopRootRegistry.Fact activeFact;

    WindowRootActivationPolicy(WindowFocusRegistry registry) { this.registry = registry; }

    /** Validate an existing grant before a new window mutation can select a gain. */
    boolean beforeWindowMutation() {
        if (activeRoot == null || fresh(activeRoot, activeFact)) return false;
        retire(activeRoot);
        return true;
    }

    /** Consumes an exact retained fact on the WMS handler, never Binder arrival order. */
    void reconcile(DesktopRootRegistry.Registration root, DesktopRootRegistry.Fact fact) {
        beforeWindowMutation();
        if (root == null || fact == null || !root.bindingOpen() || root.latestFact != fact) return;
        if (fact.kind != DesktopRootRegistry.ACTIVATED || !fresh(root, fact)) {
            retire(root);
            return;
        }
        registry.activateRoot(activation(root));
        activeRoot = root;
        activeFact = fact;
    }

    /** Exact retirement works even before BIND or after the final window is removed. */
    void retire(DesktopRootRegistry.Registration root) {
        if (root == null) return;
        registry.resignRoot(activation(root));
        if (activeRoot == root) {
            activeRoot = null;
            activeFact = null;
        }
    }

    /** Provenance of the reconciled grant, never a newer merely retained FACT. */
    DesktopRootRegistry.Fact decisionFact(DesktopRootRegistry.Registration root) {
        return activeRoot == root && activeFact != null && root.bindingOpen()
                && root.latestFact == activeFact ? activeFact : null;
    }

    private static WindowFocusRegistry.Activation activation(DesktopRootRegistry.Registration root) {
        return new WindowFocusRegistry.Activation(root, root, root.identity.pid(), 0);
    }

    private static boolean fresh(DesktopRootRegistry.Registration root, DesktopRootRegistry.Fact fact) {
        if (root == null || fact == null || !root.bindingOpen() || root.latestFact != fact
                || fact.kind != DesktopRootRegistry.ACTIVATED || !fact.keySnapshot
                || root.hostProcess == null || root.foreground == null) return false;
        try {
            root.identity.requireCaller(root.identity.pid(), root.identity.uid());
            if (!root.foreground.isForeground(root.hostProcess)) return false;
            // Native IO can overlap terminal sealing or a newer fact. Neither
            // the sampled foreground PID nor an old fact can override that.
            if (!root.bindingOpen() || root.latestFact != fact) return false;
            root.identity.requireCaller(root.identity.pid(), root.identity.uid());
            return root.bindingOpen() && root.latestFact == fact;
        } catch (RuntimeException | LinkageError failure) {
            // Rejection remains no grant; report host/attachment failures rather
            // than silently treating a failed query as successful activation.
            android.util.Log.e("WindowManager", "Desktop foreground validation rejected", failure);
            return false;
        }
    }
}
