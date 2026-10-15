// Compile-only original registration owner; checked against the pinned image.
package com.android.server.pm.resolution;
public class ComponentResolver extends ComponentResolverLocked implements com.android.server.utils.Snappable<ComponentResolverApi> {
    public ComponentResolver(com.android.server.pm.UserManagerService users, com.android.server.pm.UserNeedsBadgingCache badging) { super(users); throw new RuntimeException("stub"); }
    public ComponentResolverApi snapshot() { throw new RuntimeException("stub"); }
    public void removeAllComponents(com.android.server.pm.pkg.AndroidPackage pkg, boolean chatty) { throw new RuntimeException("stub"); }
    public void addAllComponents(com.android.server.pm.pkg.AndroidPackage pkg, boolean chatty, String setupWizard, com.android.server.pm.Computer computer) { throw new RuntimeException("stub"); }
}
