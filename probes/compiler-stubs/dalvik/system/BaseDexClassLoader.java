package dalvik.system;

import java.io.File;

/** Compile-only hidden-platform signatures; core-libart owns the runtime class. */
public class BaseDexClassLoader extends ClassLoader {
    /** Hidden API used by app_process to attach secondary dex paths. */
    public void addDexPath(String dexPath) {}

    /** Trusted variant used by framework/module loaders. */
    public void addDexPath(String dexPath, boolean isTrusted) {}

    public BaseDexClassLoader(
            String dexPath,
            File optimizedDirectory,
            String librarySearchPath,
            ClassLoader parent) {
        super(parent);
    }

    public BaseDexClassLoader(
            String dexPath, String librarySearchPath, ClassLoader parent,
            ClassLoader[] libraries) {
        this(dexPath, librarySearchPath, parent, libraries, null, false);
    }

    public BaseDexClassLoader(
            String dexPath, String librarySearchPath, ClassLoader parent,
            ClassLoader[] libraries, ClassLoader[] librariesAfter) {
        this(dexPath, librarySearchPath, parent, libraries, librariesAfter, false);
    }

    public BaseDexClassLoader(
            String dexPath, String librarySearchPath, ClassLoader parent,
            ClassLoader[] libraries, ClassLoader[] librariesAfter, boolean isTrusted) {
        super(parent);
    }
}
