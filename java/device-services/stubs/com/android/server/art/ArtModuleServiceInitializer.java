// Compile-only pinned image API; never packaged as runtime implementation.
package com.android.server.art;

public class ArtModuleServiceInitializer {
    private ArtModuleServiceInitializer() {}
    public static void setArtModuleServiceManager(android.os.ArtModuleServiceManager manager) {
        throw new RuntimeException("stub");
    }
}
