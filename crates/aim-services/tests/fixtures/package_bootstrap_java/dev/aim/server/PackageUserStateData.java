package dev.aim.server;
public final class PackageUserStateData {
    public boolean installed = true;
    public int enabled;
    public String[] on = {}, off = {};
    public String[] getEnabledComponents() { return on; }
    public String[] getDisabledComponents() { return off; }
}
