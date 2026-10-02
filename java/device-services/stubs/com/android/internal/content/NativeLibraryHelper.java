// Compile-only image API; checked by the device-services build node.
package com.android.internal.content;

public class NativeLibraryHelper {
    public static class Handle implements java.io.Closeable {
        Handle(String[] paths, long[] handles, boolean multiArch, boolean extract,
                boolean debuggable, boolean pageSizeCompatDisabled) { throw new RuntimeException("stub"); }
        public static Handle create(java.util.List<String> paths, boolean multiArch,
                boolean extract, boolean debuggable, boolean pageSizeCompatDisabled)
                throws java.io.IOException { throw new RuntimeException("stub"); }
        public void close() { throw new RuntimeException("stub"); }
    }
    public static int findSupportedAbi(Handle handle, String[] supported) { throw new RuntimeException("stub"); }
    public static boolean hasRenderscriptBitcode(Handle handle) throws java.io.IOException { throw new RuntimeException("stub"); }
    public static int copyNativeBinaries(Handle handle, java.io.File directory, String abi) { throw new RuntimeException("stub"); }
}
