package com.android.server.art;
public final class ArtManagerLocal {
    public void dump(java.io.PrintWriter text, com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot) { text.println("all dexopt"); }
    public void dumpPackage(java.io.PrintWriter text, com.android.server.pm.PackageManagerLocal.FilteredSnapshot snapshot, String name) {
        if (name.equals("absent")) throw new IllegalArgumentException("unknown package");
        if (name.equals("dead")) throw new IllegalStateException("ART died");
        text.println("dexopt " + name);
    }
}
