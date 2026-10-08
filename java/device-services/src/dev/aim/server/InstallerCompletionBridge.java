package dev.aim.server;

import com.android.server.pm.NativePackageConstants;
import android.content.Context;
import android.content.pm.PackageManager;
import android.os.BatteryManager;
import android.os.PowerManager;
import com.android.server.LocalManagerRegistry;
import com.android.server.art.ArtManagerLocal;
import com.android.server.art.model.ArtFlags;
import com.android.server.art.model.DexoptParams;
import com.android.server.pm.PackageManagerLocal;
import com.android.server.pm.dex.DexoptOptions;
/** Original ART and original device resource owners, without a PMS instance. */
public final class InstallerCompletionBridge extends IInstallerCompletionBridge.Stub {
    private final Context context;
    public InstallerCompletionBridge(Context context){this.context=java.util.Objects.requireNonNull(context);}
    private boolean critical(){
        var battery=context.getSystemService(BatteryManager.class);
        var power=context.getSystemService(PowerManager.class);
        int level=context.getResources().getInteger(com.android.internal.R.integer.config_criticalBatteryWarningLevel);
        return battery!=null&&battery.getIntProperty(BatteryManager.BATTERY_PROPERTY_STATUS)==BatteryManager.BATTERY_STATUS_DISCHARGING
            &&battery.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)<=level
            ||power!=null&&power.getCurrentThermalStatus()>=PowerManager.THERMAL_STATUS_SEVERE;
    }
    private int reason(int scenario){
        boolean critical=critical();
        switch(scenario){
            case PackageManager.INSTALL_SCENARIO_DEFAULT:return NativePackageConstants.REASON_INSTALL;
            case PackageManager.INSTALL_SCENARIO_FAST:return NativePackageConstants.REASON_INSTALL_FAST;
            case PackageManager.INSTALL_SCENARIO_BULK:return critical?NativePackageConstants.REASON_INSTALL_BULK_DOWNGRADED:NativePackageConstants.REASON_INSTALL_BULK;
            case PackageManager.INSTALL_SCENARIO_BULK_SECONDARY:return critical?NativePackageConstants.REASON_INSTALL_BULK_SECONDARY_DOWNGRADED:NativePackageConstants.REASON_INSTALL_BULK_SECONDARY;
            default:throw new IllegalArgumentException("Invalid installation scenario");
        }
    }
    @Override public int dexoptInstalled(String name,int scenario,int installReason,int flags,String filter,boolean debuggable,boolean instant,boolean apex,boolean rollback){
        enforceNativeOwner();
        if(instant||apex||rollback)return -1; // Original shouldCallArtService excludes these requests.
        var manager=LocalManagerRegistry.getManager(ArtManagerLocal.class);
        var packages=LocalManagerRegistry.getManager(PackageManagerLocal.class);
        if(manager==null||packages==null)throw new IllegalStateException("original ART/package snapshot owner unavailable");
        int dexFlags=NativePackageConstants.DEXOPT_BOOT_COMPLETE|NativePackageConstants.DEXOPT_CHECK_FOR_PROFILES_UPDATES|NativePackageConstants.DEXOPT_INSTALL_WITH_DEX_METADATA_FILE;
        if(installReason==PackageManager.INSTALL_REASON_DEVICE_RESTORE||installReason==PackageManager.INSTALL_REASON_DEVICE_SETUP)dexFlags|=NativePackageConstants.DEXOPT_FOR_RESTORE;
        var options=new DexoptOptions(name,reason(scenario),dexFlags);
        if(filter!=null)options=options.overrideCompilerFilter(filter);
        else if(debuggable)options=options.overrideCompilerFilter(DexoptParams.COMPILER_FILTER_NOOP);
        int extra=(flags&PackageManager.INSTALL_IGNORE_DEXOPT_PROFILE)!=0?ArtFlags.FLAG_IGNORE_PROFILE:0;
        try(var snapshot=packages.withFilteredSnapshot()){
            return manager.dexoptPackage(snapshot,name,options.convertToDexoptParams(extra)).getFinalStatus();
        } catch (RuntimeException failure) {
            android.util.Log.e("InstallerCompletionBridge", "Original installed-package dexopt failed: " + name, failure);
            throw failure;
        }
    }
    private static void enforceNativeOwner() {
        if (android.os.Binder.getCallingUid() != android.os.Process.SYSTEM_UID)
            throw new SecurityException("native installer bridge requires system UID");
    }
}
