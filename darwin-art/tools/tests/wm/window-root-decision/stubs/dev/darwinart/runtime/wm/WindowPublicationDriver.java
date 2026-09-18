package dev.darwinart.runtime.wm;

public final class WindowPublicationDriver {
    Throwable failure;
    void checkHealthy() {
        if (failure != null) throw new IllegalStateException("driver failed", failure);
    }
    void fail(Throwable error) { if (failure == null) failure = error; }
}
