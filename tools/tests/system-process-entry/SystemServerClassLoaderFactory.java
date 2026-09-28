package com.android.internal.os;
// Test-only substitute: a loader per standalone system-server jar.
public final class SystemServerClassLoaderFactory {
    public static dalvik.system.PathClassLoader createClassLoader(String path, ClassLoader parent) {
        return new dalvik.system.PathClassLoader();
    }
}
