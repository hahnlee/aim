package com.android.server.pm;

import android.content.Context;
import android.os.Build;
import android.os.SystemClock;
import com.android.server.SystemConfig;
import com.android.server.pm.dex.DexManager;
import com.android.server.pm.dex.ArtManagerService;
import com.android.server.pm.dex.DynamicCodeLogger;
import com.android.server.pm.permission.LegacyPermissionManagerInternal;
import com.android.server.pm.permission.LegacyPermissionManagerService;
import com.android.server.pm.permission.PermissionManagerService;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import java.io.File;
import java.util.Objects;

/** Independent original PMS injector constructors, android-16.0.0_r1. */
public final class NativePackageOriginalOwners {
    private final Context context;
    private final CompilerStats compiler = new CompilerStats();
    private final DynamicCodeLogger dynamic;
    private final DexManager dex;
    private final ArtManagerService art;
    private PermissionManagerServiceInternal permissions;
    private LegacyPermissionManagerInternal legacy;

    public NativePackageOriginalOwners(Context context, Installer installer,
            PackageManagerTracedLock installLock) {
        this.context = Objects.requireNonNull(context);
        Objects.requireNonNull(installer);
        Objects.requireNonNull(installLock);
        dynamic = new DynamicCodeLogger(installer);
        var optimizer = new PackageDexOptimizer(installer, installLock, context, "*dexopt*");
        dex = new DexManager(context, optimizer, dynamic);
        art = new ArtManagerService(context, installer, installLock);
    }
    /** Called only after original UM and the stable native PMInternal are registered. */
    public synchronized PermissionManagerServiceInternal permissions() {
        if (permissions == null) permissions = Objects.requireNonNull(
                PermissionManagerService.create(context, SystemConfig.getInstance().getAvailableFeatures()));
        return permissions;
    }
    /** InitAppsHelper's original system-overlay singleton over admitted system/factory code. */
    public static void initializeSystemOverlays(com.android.server.pm.PackageManagerLocal packages) {
        Objects.requireNonNull(packages);
        var apex=ApexManager.getInstance();
        var activeApexes=apex.getActiveApexInfos();
        var preinstalled=new java.util.HashMap<String,File>();
        try(var snapshot=packages.withUnfilteredSnapshot()){
            var selected=new java.util.LinkedHashMap<String,com.android.server.pm.pkg.PackageState>();
            snapshot.getPackageStates().forEach((name,state)->{
                var factory=snapshot.getDisabledSystemPackageStates().get(name);
                selected.put(name,state.isUpdatedSystemApp()&&factory!=null?factory:state);
            });
            for(var state:selected.values()){
                var pkg=state.getAndroidPackage();if(pkg==null||!state.isSystem())continue;
                for(var info:activeApexes){
                    if(pkg.getBaseApkPath().startsWith(info.apexDirectory.getAbsolutePath()+File.separator)){
                        String apexName=apex.getActivePackageNameForApexModuleName(info.apexModuleName);
                        if(!apex.getApksInApex(apexName).contains(pkg.getPackageName()))apex.registerApkInApex(pkg);
                        preinstalled.put(pkg.getPackageName(),info.preInstalledApexPath);
                    }
                }
            }
            com.android.internal.content.om.OverlayConfig.initializeSystemInstance(consumer->{
                for(var state:selected.values()){
                    var pkg=state.getAndroidPackage();if(pkg==null)continue;
                    var overlay=new com.android.internal.content.om.OverlayConfig.PackageProvider.Package(){
                        @Override public String getBaseApkPath(){return pkg.getBaseApkPath();}
                        @Override public int getOverlayPriority(){return pkg.getOverlayPriority();}
                        @Override public String getOverlayTarget(){return pkg.getOverlayTarget();}
                        @Override public String getPackageName(){return pkg.getPackageName();}
                        @Override public int getTargetSdkVersion(){return pkg.getTargetSdkVersion();}
                        @Override public boolean isOverlayIsStatic(){return pkg.isOverlayIsStatic();}
                    };
                    consumer.accept(overlay,state.isSystem(),preinstalled.get(pkg.getPackageName()));
                }
            });
        }
    }
    /** After admitted native Local/Internal publication, before live permission projections. */
    public static void initializePermissionState(PermissionManagerServiceInternal owner,
            com.android.server.pm.permission.LegacyPermissionSettings definitions,
            com.android.server.pm.PackageManagerLocal packages, byte[] admissions,
            boolean fingerprintChanged) {
        Objects.requireNonNull(owner).readLegacyPermissionsTEMP(Objects.requireNonNull(definitions));
        owner.readLegacyPermissionStateTEMP();
        var record=android.os.Parcel.obtain();
        try(var snapshot=Objects.requireNonNull(packages).withUnfilteredSnapshot()) {
            record.unmarshall(Objects.requireNonNull(admissions),0,admissions.length);record.setDataPosition(0);
            if(record.readInt()!=1)throw new IllegalArgumentException("Boot permission admission version differs");
            int count=record.readInt();
            if(count<=0||count>record.dataAvail()/12)throw new IllegalArgumentException("Invalid boot permission admission count");
            var accepted=new java.util.ArrayList<PermissionAdmission>();
            for(int i=0;i<count;i++) {
                String name=Objects.requireNonNull(record.readString());
                int instant=record.readInt();
                if(instant!=0&&instant!=1)throw new IllegalArgumentException("Invalid boot instant scan flag");
                byte[] oldCache=record.createByteArray();
                var state=Objects.requireNonNull(snapshot.getPackageStates().get(name),"Admitted boot permission package unavailable");
                if(state.getAndroidPackage()==null)throw new IllegalStateException("Admitted boot permission code unavailable");
                com.android.server.pm.pkg.AndroidPackage oldPackage=oldCache==null?null:
                        (com.android.server.pm.pkg.AndroidPackage)com.android.server.pm.parsing.PackageCacher.fromCacheEntryStatic(oldCache);
                accepted.add(new PermissionAdmission(state,instant!=0,oldPackage));
            }
            if(record.dataAvail()!=0)throw new IllegalArgumentException("Boot permission admission trailing data");
            for(var admission:accepted) {
                if(!admission.state().isApex())owner.onPackageAdded(admission.state(),admission.instant(),admission.oldPackage());
            }
            owner.onStorageVolumeMounted(android.os.storage.StorageManager.UUID_PRIVATE_INTERNAL,fingerprintChanged);
        } finally {record.recycle();}
    }
    private record PermissionAdmission(com.android.server.pm.pkg.PackageState state,boolean instant,
            com.android.server.pm.pkg.AndroidPackage oldPackage) {}
    public synchronized LegacyPermissionManagerInternal legacy() {
        if (legacy == null) legacy = Objects.requireNonNull(LegacyPermissionManagerService.create(context));
        return legacy;
    }
    public android.content.pm.dex.IArtManager art() { return art; }
    public CompilerStats compiler() { return compiler; }
    public DexManager dex() { return dex; }
    public DynamicCodeLogger dynamic() { return dynamic; }
    public File parserCache() {
        return PackageManagerServiceUtils.preparePackageParserCache(
                "eng".equals(Build.TYPE), "userdebug".equals(Build.TYPE), Build.VERSION.INCREMENTAL);
    }
    public NativePackageEffectsBridge.Policy effectsPolicy() {
        return new NativePackageEffectsBridge.Policy(
                com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.reduceBroadcastsForComponentStateChanges(),
                android.multiuser.Flags.enablePrivateSpaceFeatures(),
                com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.stayStopped(), SystemClock.uptimeMillis() + 60_000L);
    }
}
