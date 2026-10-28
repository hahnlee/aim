package com.android.server.art;
public final class AppHostDexUse {
    public AppHostDexUse(android.content.Context context, Object backend) {
        ArtModuleServiceInitializer.getArtModuleServiceManager();
        if (backend == null) throw new AssertionError("missing native package backend");
    }
}
