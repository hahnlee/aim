package dev.darwinart.runtime.wm;

public final class WindowRootActivationPolicy {
    private final WindowFocusRegistry registry;
    private DesktopRootRegistry.Registration root;
    private DesktopRootRegistry.Fact fact;

    WindowRootActivationPolicy(WindowFocusRegistry value) { registry = value; }
    DesktopRootRegistry.Fact decisionFact(DesktopRootRegistry.Registration selected) {
        return root == selected ? fact : null;
    }
    void activate(DesktopRootRegistry.Registration selected, long serial) {
        root = selected;
        fact = new DesktopRootRegistry.Fact(serial);
    }
}
