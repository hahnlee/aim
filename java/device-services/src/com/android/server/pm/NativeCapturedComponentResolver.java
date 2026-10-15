package com.android.server.pm;

import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ProviderInfo;
import android.content.pm.ResolveInfo;
import com.android.internal.pm.pkg.component.ParsedActivity;
import com.android.internal.pm.pkg.component.ParsedMainComponent;
import com.android.internal.pm.pkg.component.ParsedProvider;
import com.android.internal.pm.pkg.component.ParsedService;
import com.android.server.pm.resolution.ComponentResolverApi;
import dev.aim.server.PackageSnapshots;
import java.io.PrintWriter;
import java.util.List;
import java.util.Objects;

/** Original component interface, using the retained parsed originals and native registered filters. */
public final class NativeCapturedComponentResolver implements ComponentResolverApi {
    private final PackageSnapshots.ComputerSnapshot packages;
    public NativeCapturedComponentResolver(PackageSnapshots.ComputerSnapshot packages) { this.packages=Objects.requireNonNull(packages); }
    private com.android.server.pm.pkg.AndroidPackage pkg(ComponentName component) {
        var state=packages.getPackageStates().get(component.getPackageName());return state==null?null:state.getPkg();
    }
    private <T extends ParsedMainComponent> T find(ComponentName component,List<T> entries) {
        if(entries!=null)for(T entry:entries)if(entry.getName().equals(component.getClassName()))return entry;return null;
    }
    @Override public ParsedActivity getActivity(ComponentName component){var pkg=pkg(component);return pkg==null?null:find(component,pkg.getActivities());}
    @Override public ParsedActivity getReceiver(ComponentName component){var pkg=pkg(component);return pkg==null?null:find(component,pkg.getReceivers());}
    @Override public ParsedService getService(ComponentName component){var pkg=pkg(component);return pkg==null?null:find(component,pkg.getServices());}
    @Override public ParsedProvider getProvider(ComponentName component){var pkg=pkg(component);return pkg==null?null:find(component,pkg.getProviders());}
    @Override public boolean componentExists(ComponentName component){return getActivity(component)!=null||getReceiver(component)!=null||getService(component)!=null||getProvider(component)!=null;}
    private List<ResolveInfo> query(int kind,Intent intent,String type,long flags,List<? extends ParsedMainComponent> subset,int user) {
        ComponentName[] names=null;
        if(subset!=null){names=new ComponentName[subset.size()];for(int i=0;i<names.length;i++){
            var entry=subset.get(i);ComponentName name=new ComponentName(entry.getPackageName(),entry.getName());
            ParsedMainComponent retained=switch(kind){case 0->getActivity(name);case 1->getReceiver(name);case 2->getService(name);case 3->getProvider(name);default->throw new IllegalArgumentException("component kind");};
            if(retained!=entry)throw new IllegalArgumentException("component is outside retained registration");names[i]=name;
        }}
        return packages.queryRawComponents(kind,intent,type,flags,names,user);
    }
    @Override public List<ResolveInfo> queryActivities(Computer computer,Intent intent,String type,long flags,int user){return query(0,intent,type,flags,null,user);}
    @Override public List<ResolveInfo> queryActivities(Computer computer,Intent intent,String type,long flags,List<ParsedActivity> subset,int user){return query(0,intent,type,flags,subset,user);}
    @Override public List<ResolveInfo> queryReceivers(Computer computer,Intent intent,String type,long flags,int user){return query(1,intent,type,flags,null,user);}
    @Override public List<ResolveInfo> queryReceivers(Computer computer,Intent intent,String type,long flags,List<ParsedActivity> subset,int user){return query(1,intent,type,flags,subset,user);}
    @Override public List<ResolveInfo> queryServices(Computer computer,Intent intent,String type,long flags,int user){return query(2,intent,type,flags,null,user);}
    @Override public List<ResolveInfo> queryServices(Computer computer,Intent intent,String type,long flags,List<ParsedService> subset,int user){return query(2,intent,type,flags,subset,user);}
    @Override public List<ResolveInfo> queryProviders(Computer computer,Intent intent,String type,long flags,int user){return query(3,intent,type,flags,null,user);}
    @Override public List<ResolveInfo> queryProviders(Computer computer,Intent intent,String type,long flags,List<ParsedProvider> subset,int user){return query(3,intent,type,flags,subset,user);}
    @Override public ProviderInfo queryProvider(Computer computer,String authority,long flags,int user){return packages.queryRawProvider(authority,flags,user);}
    @Override public List<ProviderInfo> queryProviders(Computer computer,String process,String metadata,int uid,long flags,int user){return packages.queryRawProviders(process,metadata,uid,flags,user);}
    @Override public void querySyncProviders(Computer computer,List<String> names,List<ProviderInfo> providers,boolean safeMode,int user){packages.queryRawSyncProviders(names,providers,safeMode,user);}
    @Override public void dumpActivityResolvers(PrintWriter writer,DumpState state,String name){packages.dumpRawComponents(0,writer,state,name);}
    @Override public void dumpReceiverResolvers(PrintWriter writer,DumpState state,String name){packages.dumpRawComponents(1,writer,state,name);}
    @Override public void dumpServiceResolvers(PrintWriter writer,DumpState state,String name){packages.dumpRawComponents(2,writer,state,name);}
    @Override public void dumpProviderResolvers(PrintWriter writer,DumpState state,String name){packages.dumpRawComponents(3,writer,state,name);}
    @Override public void dumpContentProviders(Computer computer,PrintWriter writer,DumpState state,String name){packages.dumpRawComponents(4,writer,state,name);}
    @Override public void dumpServicePermissions(PrintWriter writer,DumpState state){packages.dumpRawComponents(5,writer,state,null);}
}
