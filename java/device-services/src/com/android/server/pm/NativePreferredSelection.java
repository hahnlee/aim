package com.android.server.pm;

import android.content.Context;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ResolveInfo;
import dev.aim.server.PackageSnapshots;
import java.util.List;
import java.util.Objects;

/** Original result container; registry edits are committed by its native retained owner. */
public final class NativePreferredSelection {
    private final Context context;
    private final PackageSnapshots.ComputerSnapshot packages;
    public NativePreferredSelection(Context context,PackageSnapshots.ComputerSnapshot packages){this.context=Objects.requireNonNull(context);this.packages=Objects.requireNonNull(packages);}
    public PackageManagerService.FindPreferredActivityBodyResult find(Computer computer,Intent intent,
            String type,long flags,List<ResolveInfo> query,boolean always,boolean remove,boolean debug,int user,boolean filtered){
        boolean provisioned=android.provider.Settings.Global.getInt(context.getContentResolver(),"device_provisioned",0)==1;
        flags=computer.updateFlagsForResolve(flags,user,android.os.Binder.getCallingUid(),false,
                computer.isImplicitImageCaptureIntentAndNotSetByDpc(intent,user,type,flags));
        intent=PackageManagerServiceUtils.updateIntentForResolve(intent);
        var result=new PackageManagerService.FindPreferredActivityBodyResult();
        result.mPreferredResolveInfo=computer.findPersistentPreferredActivity(intent,type,flags,query,debug,user);
        if(result.mPreferredResolveInfo!=null)return result;
        ComponentName[] names=new ComponentName[query.size()];int[] matches=new int[query.size()];
        for(int i=0;i<query.size();i++){var info=query.get(i);names[i]=new ComponentName(info.activityInfo.applicationInfo.packageName,info.activityInfo.name);matches[i]=info.match;}
        int[] chosen=packages.selectPreferredActivity(intent,type,flags,names,matches,always,remove,filtered,provisioned,user);
        if(chosen==null||chosen.length!=2||chosen[1]<-1||chosen[1]>=query.size())throw new IllegalStateException("native preferred selection record differs");
        result.mChanged=chosen[0]!=0;result.mPreferredResolveInfo=chosen[1]<0?null:query.get(chosen[1]);return result;
    }
}
