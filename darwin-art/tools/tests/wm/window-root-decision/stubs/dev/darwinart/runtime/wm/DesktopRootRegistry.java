package dev.darwinart.runtime.wm;

public final class DesktopRootRegistry {
    static final class BindingRejected extends SecurityException {
        BindingRejected(String message) { super(message); }
    }

    static final class Fact {
        final long serial;
        Fact(long value) { serial = value; }
    }

    static final class Registration {
        final long incarnation;
        Fact latestFact;
        boolean clientDied;
        boolean terminal;
        boolean published = true;

        Registration(long rootIncarnation) { incarnation = rootIncarnation; }
        boolean bindingOpen() { return !terminal && published; }
    }

    private DesktopRootRegistry() {}
}
