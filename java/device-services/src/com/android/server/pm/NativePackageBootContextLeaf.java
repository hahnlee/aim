package com.android.server.pm;

import android.content.Context;
import android.os.Binder;
import android.os.Parcel;
import android.provider.Settings;
import com.android.server.LocalServices;
import dev.aim.server.IPackageBootContextLeaf;

/** Real original UM and Resources inputs, without PackageManager feed/action delegation. */
public final class NativePackageBootContextLeaf extends IPackageBootContextLeaf.Stub {
    private final Context context;
    private volatile boolean ready;
    private dev.aim.server.IPackageFilteringUpdates updates;
    private volatile boolean queryDisabled=!android.content.pm.PackageManager.APP_ENUMERATION_ENABLED_BY_DEFAULT;
    public NativePackageBootContextLeaf(Context context) {this.context=java.util.Objects.requireNonNull(context);}
    @Override public byte[] captureBootContext() {
        if(Binder.getCallingUid()!=1000)throw new SecurityException("Native boot context owner required");
        var users=UserManagerService.getInstance();
        var userState=LocalServices.getService(UserManagerInternal.class);
        if(users==null||userState==null)throw new IllegalStateException("Original UM context owner unavailable");
        var resources=context.getResources();
        var out=Parcel.obtain();long identity=Binder.clearCallingIdentity();
        try {
            out.writeInt(2);
            var infos=java.util.Objects.requireNonNull(users.getUsers(true,false,false));out.writeInt(infos.size());
            for(var info:infos) {out.writeInt(info.id);out.writeInt(info.flags);out.writeBoolean(info.preCreated);
                out.writeInt(info.profileGroupId);out.writeBoolean(userState.isUserUnlockingOrUnlocked(info.id));}
            out.writeInt(com.android.internal.R.style.Theme_Material_Dialog_Alert);
            String[] actions={android.content.Intent.ACTION_VIEW,android.content.Intent.ACTION_EDIT,
                android.content.Intent.ACTION_SEND,android.content.Intent.ACTION_SENDTO,
                android.content.Intent.ACTION_SEND_MULTIPLE,android.provider.MediaStore.ACTION_IMAGE_CAPTURE,
                null,android.content.Intent.ACTION_MAIN};
            int[] titles={com.android.internal.R.string.whichViewApplicationLabel,com.android.internal.R.string.whichEditApplicationLabel,
                com.android.internal.R.string.whichSendApplicationLabel,com.android.internal.R.string.whichSendToApplicationLabel,
                com.android.internal.R.string.whichSendApplicationLabel,com.android.internal.R.string.whichImageCaptureApplicationLabel,
                com.android.internal.R.string.whichApplicationLabel,com.android.internal.R.string.whichHomeApplicationLabel};
            out.writeInt(actions.length);for(int i=0;i<actions.length;i++){out.writeString(actions[i]);out.writeInt(titles[i]);}
            out.writeString(resources.getString(com.android.internal.R.string.config_customResolverActivity));
            // AppsFilter.FeatureConfigImpl constructor and original parser state;
            // neither reads a provider before PMS.systemReady.
            out.writeBoolean(queryDisabled);
            out.writeBoolean(com.android.internal.pm.pkg.parsing.ParsingPackageUtils.sCompatibilityModeEnabled);
            return out.marshall();
        } finally {Binder.restoreCallingIdentity(identity);out.recycle();}
    }
    private static void enforceOwner() {if(Binder.getCallingUid()!=1000)throw new SecurityException("Native boot context owner required");}
    @Override public boolean userUnlockingOrUnlocked(int userId) {
        enforceOwner();
        return java.util.Objects.requireNonNull(LocalServices.getService(UserManagerInternal.class),
            "Original UserManagerInternal owner unavailable").isUserUnlockingOrUnlocked(userId);
    }
    @Override public boolean deviceProvisioned() {
        enforceOwner();if(!ready)throw new IllegalStateException("SettingsProvider phase has not reached PMS.systemReady");
        return Settings.Global.getInt(context.getContentResolver(),Settings.Global.DEVICE_PROVISIONED,0)!=0;
    }
    @Override public boolean queryFilteringDisabled() {enforceOwner();return queryDisabled;}
    @Override public synchronized void beginSystemReady() {
        enforceOwner();
        if(!ready) {
            queryDisabled=!android.provider.DeviceConfig.getBoolean(android.provider.DeviceConfig.NAMESPACE_PACKAGE_MANAGER_SERVICE,
                "package_query_filtering_enabled",android.content.pm.PackageManager.APP_ENUMERATION_ENABLED_BY_DEFAULT);
            android.provider.DeviceConfig.addOnPropertiesChangedListener(android.provider.DeviceConfig.NAMESPACE_PACKAGE_MANAGER_SERVICE,
                com.android.server.FgThread.getExecutor(),properties->{if(properties.getKeyset().contains("package_query_filtering_enabled"))
                    publishFiltering(!properties.getBoolean("package_query_filtering_enabled",android.content.pm.PackageManager.APP_ENUMERATION_ENABLED_BY_DEFAULT));});
            ready=true;publishFiltering(queryDisabled);
        }
    }
    @Override public synchronized boolean enterSystemReady() {
        enforceOwner();
        if(!ready)throw new IllegalStateException("PMS filtering readiness has not begun");
        boolean compatibility=Settings.Global.getInt(context.getContentResolver(),Settings.Global.COMPATIBILITY_MODE,1)==1;
        com.android.internal.pm.pkg.parsing.ParsingPackageUtils.setCompatibilityModeEnabled(compatibility);
        return compatibility;
    }
    private synchronized void publishFiltering(boolean disabled) {
        queryDisabled=disabled;
        if(updates!=null)try {updates.changed(disabled);}catch(android.os.RemoteException failure) {throw new IllegalStateException("Native filtering publication owner unavailable",failure);}
    }
    @Override public synchronized void registerFilteringUpdates(dev.aim.server.IPackageFilteringUpdates owner) {
        enforceOwner();updates=java.util.Objects.requireNonNull(owner);publishFiltering(queryDisabled);
    }
}
