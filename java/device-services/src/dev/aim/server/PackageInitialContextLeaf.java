package dev.aim.server;

/** Independent original build, permission and policy owners before native capture. */
public final class PackageInitialContextLeaf extends IPackageInitialContextLeaf.Stub {
    private final android.content.Context context;
    private static void enforce(){if(android.os.Binder.getCallingUid()!=1000)throw new SecurityException("Native initial context owner required");}
    public PackageInitialContextLeaf(android.content.Context context){this.context=java.util.Objects.requireNonNull(context);}
    @Override public String getLiveProperty(String name){enforce();return android.os.SystemProperties.get(java.util.Objects.requireNonNull(name));}
    @Override public byte[] captureImageScanInputs(){
        enforce();var out=android.os.Parcel.obtain();
        try{out.writeInt(1);out.writeStringArray(android.os.Build.SUPPORTED_ABIS);out.writeStringArray(android.os.Build.SUPPORTED_32_BIT_ABIS);out.writeStringArray(android.os.Build.SUPPORTED_64_BIT_ABIS);
            out.writeBoolean(dalvik.system.VMRuntime.getRuntime().is64Bit());out.writeString(new java.io.File(android.os.Environment.getDataDirectory(),"app-lib").toString());
            out.writeLong(com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.fixSystemAppsFirstInstallTime()?java.lang.System.currentTimeMillis():0);out.writeLong(android.system.Os.sysconf(android.system.OsConstants._SC_PAGESIZE));
            out.writeBoolean(android.os.Build.isDebuggable());return out.marshall();
        }finally{out.recycle();}
    }
    @Override public byte[] getPermissionGroup(String name,int flags)throws android.os.RemoteException{
        enforce();var owner=android.permission.IPermissionManager.Stub.asInterface(java.util.Objects.requireNonNull(android.os.ServiceManager.checkService("permissionmgr")));
        var info=owner.getPermissionGroupInfo(name,flags);if(info==null)return null;var out=android.os.Parcel.obtain();try{info.writeToParcel(out,0);return out.marshall();}finally{out.recycle();}
    }
    @Override public boolean isInstallDisabled(String name,int uid,int user){
        enforce();var users=java.util.Objects.requireNonNull(com.android.server.pm.UserManagerService.getInstance());
        if(users.hasUserRestriction("no_install_unknown_sources",user)||users.hasUserRestriction("no_install_unknown_sources_globally",user))return true;
        return InstallerExternalBridge.isExternalSourceDisabled(name,uid);
    }
    @Override public int getAutoRevoke(String name,int uid){enforce();return java.util.Objects.requireNonNull(context.getSystemService(android.app.AppOpsManager.class)).checkOpNoThrow(android.app.AppOpsManager.OP_AUTO_REVOKE_PERMISSIONS_IF_UNUSED,uid,name);}
    @Override public int getInstallLocation(){enforce();return android.provider.Settings.Global.getInt(context.getContentResolver(),"default_install_location",0);}
    @Override public byte[] getAdminFacts(String name)throws android.os.RemoteException{
        enforce();var binder=android.os.ServiceManager.checkService("device_policy");
        var dpm=binder==null?null:android.app.admin.IDevicePolicyManager.Stub.asInterface(binder);
        var internal=com.android.server.LocalServices.getService(android.app.admin.DevicePolicyManagerInternal.class);
        var users=java.util.Objects.requireNonNull(com.android.server.pm.UserManagerService.getInstance());
        var out=android.os.Parcel.obtain();long identity=android.os.Binder.clearCallingIdentity();
        try{boolean present=dpm!=null&&internal!=null;out.writeBoolean(present);
            var deviceOwner=present?dpm.getDeviceOwnerComponent(false):null;out.writeString(deviceOwner==null?null:deviceOwner.getPackageName());
            var active=new java.util.ArrayList<Integer>();var managed=new java.util.ArrayList<Integer>();
            if(present){var roles=java.util.Objects.requireNonNull(context.getSystemService(android.app.role.RoleManager.class));
                for(int user:users.getUserIds()){
                    if(dpm.packageHasActiveAdmins(name,user))active.add(user);
                    var holders=roles.getRoleHoldersAsUser(android.app.role.RoleManager.ROLE_DEVICE_POLICY_MANAGEMENT,android.os.UserHandle.of(user));
                    if(!holders.isEmpty()&&java.util.Objects.equals(name,holders.get(0))&&internal.isUserOrganizationManaged(user))managed.add(user);
                }
            }
            out.writeIntArray(active.stream().mapToInt(Integer::intValue).toArray());out.writeIntArray(managed.stream().mapToInt(Integer::intValue).toArray());return out.marshall();
        }finally{android.os.Binder.restoreCallingIdentity(identity);out.recycle();}
    }
    @Override public String[] getMountedPrivateVolumes(){
        enforce();var owner=java.util.Objects.requireNonNull(context.getSystemService(android.os.storage.StorageManager.class));
        var result=new java.util.ArrayList<String>();for(var volume:owner.getVolumes())if(volume.getType()==android.os.storage.VolumeInfo.TYPE_PRIVATE&&volume.isMountedReadable()&&volume.getFsUuid()!=null)result.add(volume.getFsUuid());
        return result.toArray(new String[0]);
    }
    @Override public boolean isIncrementalPath(String path){enforce();return android.os.incremental.IncrementalManager.isIncrementalPath(path);}
    @Override public long zipEntryTime(int dosTime){
        enforce();long value=Integer.toUnsignedLong(dosTime);
        return java.time.LocalDateTime.of((int)((value>>>25)+1980),(int)((value>>>21)&15),(int)((value>>>16)&31),(int)((value>>>11)&31),(int)((value>>>5)&63),(int)((value&31)*2)).atZone(java.time.ZoneId.systemDefault()).toInstant().toEpochMilli();
    }
    @Override public byte[] nativeLibraryInode(String path)throws android.os.RemoteException{
        enforce();try{var stat=android.system.Os.stat(path);var out=android.os.Parcel.obtain();try{out.writeInt(stat.st_uid);out.writeInt(stat.st_gid);out.writeInt(stat.st_mode&07777);return out.marshall();}finally{out.recycle();}}
        catch(android.system.ErrnoException failure){throw new android.os.RemoteException(failure.toString());}
    }
    @Override public String getSdkSandboxServiceAction(){enforce();return com.android.server.sdksandbox.SdkSandboxManagerLocal.SERVICE_INTERFACE;}
    @Override public byte[] getLiveProperties(String[] names){
        enforce();java.util.Objects.requireNonNull(names);var out=android.os.Parcel.obtain();
        try{out.writeInt(1);out.writeInt(names.length);for(String name:names){java.util.Objects.requireNonNull(name);out.writeString(name);out.writeString(android.os.SystemProperties.get(name));}return out.marshall();}
        finally{out.recycle();}
    }
    @Override public String[] getBootClassPathApexModules(){
        enforce();String classpath=java.lang.System.getenv("BOOTCLASSPATH");
        if(classpath==null||classpath.isEmpty())throw new IllegalStateException("Original BOOTCLASSPATH absent");
        var modules=new java.util.LinkedHashSet<String>();
        for(String entry:classpath.split(":")){var path=java.nio.file.Paths.get(entry);if(path.getNameCount()>=2&&path.getName(0).toString().equals("apex"))modules.add(path.getName(1).toString());}
        return modules.toArray(new String[0]);
    }
}
