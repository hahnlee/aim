package com.android.server.pm;

import android.content.Context;
import android.content.Intent;
import android.content.pm.ResolveInfo;
import com.android.server.pm.pkg.AndroidPackage;
import dev.aim.server.PackageSnapshots;
import dev.aim.server.NativePMResolveFlags;
import java.util.List;
import java.util.Objects;

/** Complete remaining computed Computer owner over native capture and original system leaves. */
public final class NativeResolutionOwner implements NativeComputer.ResolutionOwner {
    private final PackageSnapshots.ComputerSnapshot packages;
    private final NativePMResolveFlags flags;
    private final NativePreferredSelection preferred;
    private final String platform;
    private Computer computer;
    public NativeResolutionOwner(Context context,PackageSnapshots.ComputerSnapshot packages,
            long version,boolean safeMode,UserManagerInternal users,String platformPackage) {
        this.packages=Objects.requireNonNull(packages);this.platform=Objects.requireNonNull(platformPackage);
        flags=new NativePMResolveFlags(packages,version,safeMode,users);
        preferred=new NativePreferredSelection(context,packages);
    }
    public synchronized void attach(Computer computer){if(this.computer!=null)throw new IllegalStateException("resolution view already attached");this.computer=Objects.requireNonNull(computer);}
    @Override public CrossProfileDomainInfo getCrossProfileDomainPreferredLpr(Intent intent,String type,long flags,int source,int parent){
        Integer level=packages.crossProfileDomainApproval(intent,type,flags,source,parent);if(level==null)return null;
        return new CrossProfileDomainInfo(createForwardingResolveInfoUnchecked(new WatchedIntentFilter(),source,parent),level,parent);
    }
    @Override public ResolveInfo createForwardingResolveInfoUnchecked(WatchedIntentFilter filter,int source,int target){return NativeForwardingResolveInfo.create(packages,platform,filter,source,target);}
    @Override public long updateFlagsForResolve(long value,int user,int uid,boolean instant,boolean capture){return flags.update(value,user,uid,instant,capture);}
    @Override public PackageManagerService.FindPreferredActivityBodyResult findPreferredActivityInternal(Intent intent,String type,long flags,List<ResolveInfo> query,boolean always,boolean remove,boolean debug,int user,boolean filtered){return preferred.find(Objects.requireNonNull(computer,"native resolution view unattached"),intent,type,flags,query,always,remove,debug,user,filtered);}
}
