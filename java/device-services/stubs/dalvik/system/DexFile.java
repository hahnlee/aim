// Compile-only pinned image API; never packaged as runtime implementation.
package dalvik.system;
public final class DexFile {
    public DexFile(String path) throws java.io.IOException { throw new RuntimeException("stub"); }
    public static native boolean isProfileGuidedCompilerFilter(String filter);
}
