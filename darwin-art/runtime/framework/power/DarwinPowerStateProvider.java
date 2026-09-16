package dev.darwinart.runtime.power;

/** macOS power-state seam; native registration supplies the host policy. */
public final class DarwinPowerStateProvider implements PowerStateProvider {
    @Override
    public boolean isInteractive() {
        return nativeIsInteractive();
    }

    @Override
    public boolean isDisplayInteractive(int displayId) {
        // The desktop profile currently exposes one host interactive state to
        // every logical Android display. The provider owns that policy.
        return nativeIsInteractive();
    }

    private static native boolean nativeIsInteractive();
}
