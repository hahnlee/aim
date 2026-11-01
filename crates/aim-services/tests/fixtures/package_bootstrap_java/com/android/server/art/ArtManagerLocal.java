package com.android.server.art;
public final class ArtManagerLocal {
    public static boolean missingDependency;
    public static int constructions, calls;
    public static java.util.function.BiFunction<com.android.server.pm.PackageManagerLocal.FilteredSnapshot, String, Long> deletion;
    public ArtManagerLocal() {}
    public ArtManagerLocal(android.content.Context context) {
        java.util.Objects.requireNonNull(context);
        if (missingDependency) throw new NullPointerException("missing original constructor dependency");
        constructions++;
    }
    public void clearAppProfiles(com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot, String name) {
        calls++; if (name.equals("error")) throw new IllegalArgumentException("original profile error");
    }
    public com.android.server.art.model.DeleteResult deleteDexoptArtifacts(com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot, String name) {
        calls++; if (name.equals("error")) throw new IllegalStateException("original deletion error");
        if (deletion != null) {
            long bytes = deletion.apply(snapshot, name);
            return new com.android.server.art.model.DeleteResult() { public long getFreedBytes() { return bytes; } };
        }
        return new com.android.server.art.model.DeleteResult() { public long getFreedBytes() { return 1L << 40; } };
    }
    public void dump(java.io.PrintWriter text, com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot) { text.println("all dexopt"); }
    public void dumpPackage(java.io.PrintWriter text, com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot, String name) {
        if (name.equals("absent")) throw new IllegalArgumentException("unknown package");
        if (name.equals("dead")) throw new IllegalStateException("ART died");
        text.println("dexopt " + name);
    }
}
