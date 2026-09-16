package dev.darwinart.runtime.power;

/** Host-owned interactive-state source consumed by the Android power Binder endpoint. */
public interface PowerStateProvider {
    boolean isInteractive();

    boolean isDisplayInteractive(int displayId);
}
