package dev.aim.server;

import com.android.server.pm.pkg.PackageState;
import com.android.server.pm.pkg.SharedUserApi;
import java.io.IOException;
import java.util.LinkedHashMap;
import java.util.Objects;

/** Admitted metadata only, before the original permission service initializes. */
final class NativeInitialPackageSnapshot {
    private NativeInitialPackageSnapshot() {}
    record Captured(long version,PackageSnapshots.Data data) {}
    static Captured capture(IPackageBootSession boot,boolean crossUser) throws android.os.RemoteException,IOException {
        IPackageScanSnapshot endpoint=Objects.requireNonNull(boot.getInitialMetadataSnapshot());
        try(var lease=new PackageScanLease(endpoint)){
            var active=packages(lease,endpoint,false,crossUser);var disabled=packages(lease,endpoint,true,crossUser);
            var shared=new LinkedHashMap<String,SharedUserApi>();
            for(String name:lease.getSharedUserNames()){
                var state=Objects.requireNonNull(lease.getSharedUserReplica(name,crossUser),"Initial shared UID owner absent");
                if(shared.put(name,state)!=null)throw new IOException("Duplicate initial shared UID owner");
            }
            long version=lease.getVersion();
            var filters=new PackageSnapshots.Owner(){
                private void check(long requested){if(requested!=version)throw new IllegalStateException("Initial metadata epoch differs");}
                @Override public String getFilteredPackageName(long requested,String name,int uid,int user){check(requested);try{return boot.filterInitialPackageAccess(name,uid,user,true)?null:name;}catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}}
                @Override public boolean shouldFilter(long requested,PackageState state,int uid,int user){check(requested);try{return boot.filterInitialPackageAccess(state.getPackageName(),uid,user,true);}catch(android.os.RemoteException failure){throw failure.rethrowFromSystemServer();}}
            };
            return new Captured(version,new PackageSnapshots.Data(version,active,disabled,shared,filters));
        }
    }
    private static LinkedHashMap<String,PackageState> packages(PackageScanLease lease,IPackageScanSnapshot endpoint,boolean disabled,boolean crossUser)throws android.os.RemoteException,IOException{
        var result=new LinkedHashMap<String,PackageState>();
        for(String name:endpoint.getPackageNames(disabled)){
            var state=lease.getPackageStateReplica(name,disabled,crossUser);
            if(state==null||!name.equals(state.getPackageName())||result.put(name,state)!=null)throw new IOException("Initial package metadata identity differs");
        }
        return result;
    }
}
