package com.android.internal.pm.parsing.pkg;
public record PackageImpl(String name) implements com.android.server.pm.pkg.AndroidPackage {
    public String getPackageName() { return name; }
    public String getPath() { return "/data/app/" + name; }
}
