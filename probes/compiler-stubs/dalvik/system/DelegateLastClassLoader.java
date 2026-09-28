package dalvik.system;

/** Compile-only hidden-platform signatures; core-libart owns the runtime class. */
public final class DelegateLastClassLoader extends PathClassLoader {
    public DelegateLastClassLoader(String dexPath, ClassLoader parent) {
        this(dexPath, null, parent, true);
    }

    public DelegateLastClassLoader(
            String dexPath, String librarySearchPath, ClassLoader parent) {
        this(dexPath, librarySearchPath, parent, true);
    }

    public DelegateLastClassLoader(
            String dexPath, String librarySearchPath, ClassLoader parent,
            boolean delegateResourceLoading) {
        super(dexPath, librarySearchPath, parent);
    }

    public DelegateLastClassLoader(
            String dexPath, String librarySearchPath, ClassLoader parent,
            ClassLoader[] sharedLibraryLoaders) {
        this(dexPath, librarySearchPath, parent, sharedLibraryLoaders, null);
    }

    public DelegateLastClassLoader(
            String dexPath, String librarySearchPath, ClassLoader parent,
            ClassLoader[] sharedLibraryLoaders, ClassLoader[] sharedLibraryLoadersAfter) {
        super(dexPath, librarySearchPath, parent,
                sharedLibraryLoaders, sharedLibraryLoadersAfter);
    }
}
