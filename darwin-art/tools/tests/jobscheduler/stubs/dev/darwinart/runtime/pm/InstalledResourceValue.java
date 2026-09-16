package dev.darwinart.runtime.pm;

import android.os.Bundle;

final class InstalledResourceValue {
    private InstalledResourceValue() {}
    static void put(Bundle bundle, String name, InstalledPackageRecord installed, int resourceId) {
        throw new AssertionError("resource metadata is outside this host test");
    }
}
