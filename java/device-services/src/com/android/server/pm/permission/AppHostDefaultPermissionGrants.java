// Copyright (C) The Android Open Source Project, Apache License 2.0.
package com.android.server.pm.permission;

import android.content.Context;
import dev.aim.server.IPackageDefaultGrantQueries;
import dev.aim.server.PackageLocalOwners;

/** PMS.systemReady's synchronous default grants, over the native permission Binder owner. */
public final class AppHostDefaultPermissionGrants {
    private final DefaultPermissionGrantPolicy policy;

    public AppHostDefaultPermissionGrants(Context context, IPackageDefaultGrantQueries queries) {
        PackageLocalOwners.prepareDefaultGrants(queries);
        policy = publish(context).policy;
    }

    public static synchronized Local publish(Context context) {
        var current = com.android.server.LocalServices.getService(LegacyPermissionManagerInternal.class);
        if (current instanceof Local local) return local;
        if (current != null) throw new IllegalStateException("foreign legacy permission local owner installed");
        var local = new Local(new DefaultPermissionGrantPolicy(context));
        com.android.server.LocalServices.addService(LegacyPermissionManagerInternal.class, local);
        return local;
    }

    /** One original grant policy; permission reads and mutations use its native PM Binder. */
    public static final class Local implements LegacyPermissionManagerInternal {
        private final DefaultPermissionGrantPolicy policy;
        private Local(DefaultPermissionGrantPolicy policy) { this.policy = policy; }
        @Override public void setSyncAdapterPackagesProvider(SyncAdapterPackagesProvider provider) {
            policy.setSyncAdapterPackagesProvider(provider);
        }
        @Override public void setLocationPackagesProvider(PackagesProvider provider) {
            policy.setLocationPackagesProvider(provider);
        }
        @Override public void setLocationExtraPackagesProvider(PackagesProvider provider) {
            policy.setLocationExtraPackagesProvider(provider);
        }
        @Override public void grantDefaultPermissions(int user) { policy.grantDefaultPermissions(user); }
        private UnsupportedOperationException unsupported() {
            return new UnsupportedOperationException("outside app-host default grant methods (#2410)");
        }
        @Override public void resetRuntimePermissions() { throw unsupported(); }
        @Override public void scheduleReadDefaultPermissionExceptions() { throw unsupported(); }
        @Override public void setDialerAppPackagesProvider(PackagesProvider p) { throw unsupported(); }
        @Override public void setSimCallManagerPackagesProvider(PackagesProvider p) { throw unsupported(); }
        @Override public void setSmsAppPackagesProvider(PackagesProvider p) { throw unsupported(); }
        @Override public void setUseOpenWifiAppPackagesProvider(PackagesProvider p) { throw unsupported(); }
        @Override public void setVoiceInteractionPackagesProvider(PackagesProvider p) { throw unsupported(); }
        @Override public void grantDefaultPermissionsToDefaultSimCallManager(String name, int user) { throw unsupported(); }
        @Override public void grantDefaultPermissionsToDefaultUseOpenWifiApp(String name, int user) { throw unsupported(); }
        @Override public int checkSoundTriggerRecordAudioPermissionForDataDelivery(int uid, String name, String tag, String reason) { throw unsupported(); }
    }

    public void grant(int userId) {
        policy.grantDefaultPermissions(userId);
    }
}
