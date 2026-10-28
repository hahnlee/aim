package com.android.server.pm;
public class PackageDexOptimizer {
    public PackageDexOptimizer(Installer installer, PackageManagerTracedLock lock, android.content.Context context, String tag) {
        if (installer == null || lock == null || context == null || !tag.equals("*dexopt*")) throw new AssertionError();
    }
}
