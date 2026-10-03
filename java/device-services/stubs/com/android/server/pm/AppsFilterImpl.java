// Compile-only pinned image API; checked by the device-services build node.
package com.android.server.pm;
public final class AppsFilterImpl extends AppsFilterLocked {
    AppsFilterImpl(FeatureConfig config, String[] packages, boolean systemQueryable,
        com.android.server.om.OverlayReferenceMapper.Provider provider, android.os.Handler handler) {
        throw new RuntimeException("stub");
    }
    public boolean grantImplicitAccess(int recipient, int visible, boolean retain) { throw new RuntimeException("stub"); }
}
