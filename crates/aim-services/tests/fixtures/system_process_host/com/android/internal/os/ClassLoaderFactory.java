package com.android.internal.os;
public final class ClassLoaderFactory {
    public static ClassLoader createClassLoader(String path, String libraryPath,
            String permittedPath, ClassLoader parent, int sdk, boolean shared, String name) {
        return ClassLoaderFactory.class.getClassLoader();
    }
}
