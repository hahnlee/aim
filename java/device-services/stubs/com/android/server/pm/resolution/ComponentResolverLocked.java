// Compile-only original image hierarchy.
package com.android.server.pm.resolution;
public abstract class ComponentResolverLocked extends ComponentResolverBase {
    protected ComponentResolverLocked(com.android.server.pm.UserManagerService users) { super(users); }
    public com.android.internal.pm.pkg.component.ParsedProvider getProvider(android.content.ComponentName component) { throw new RuntimeException("stub"); }
}
