package dev.darwinart.runtime.input;

import android.view.KeyCharacterMap;

/** Android system input-device map selection, not host character translation. */
final class SystemKeyboardMaps {
    private SystemKeyboardMaps() {}

    static KeyCharacterMap load(int deviceId) {
        final String path;
        if (deviceId == InputDeviceRegistry.PHYSICAL_KEYBOARD_ID) {
            // The current Darwin seat normalizes physical positions to a US
            // keyboard. Host-layout overlays are a separate provider contract.
            path = "/system/usr/keychars/Generic.kcm";
        } else if (deviceId == InputDeviceRegistry.VIRTUAL_KEYBOARD_ID) {
            path = "/system/usr/keychars/Virtual.kcm";
        } else {
            throw new IllegalArgumentException("Unknown system keyboard: " + deviceId);
        }
        return nativeLoad(deviceId, path);
    }

    private static native KeyCharacterMap nativeLoad(int deviceId, String path);
}
