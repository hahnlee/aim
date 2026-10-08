// Compile-only pinned original full interface.
package com.android.server.pm.resolution;
import android.content.ComponentName;
import android.content.Intent;
import android.content.pm.ProviderInfo;
import android.content.pm.ResolveInfo;
import com.android.internal.pm.pkg.component.ParsedActivity;
import com.android.internal.pm.pkg.component.ParsedProvider;
import com.android.internal.pm.pkg.component.ParsedService;
import com.android.server.pm.Computer;
import com.android.server.pm.DumpState;
import java.io.PrintWriter;
import java.util.List;
public abstract class ComponentResolverBase extends com.android.server.utils.WatchableImpl implements ComponentResolverApi {
    protected ComponentResolverBase(com.android.server.pm.UserManagerService users) { throw new RuntimeException("stub"); }
    public boolean isActivityDefined(ComponentName component) { throw new RuntimeException("stub"); }
    public ParsedActivity getActivity(ComponentName component) { throw new RuntimeException("stub"); }
    public ParsedProvider getProvider(ComponentName component) { throw new RuntimeException("stub"); }
    public ParsedActivity getReceiver(ComponentName component) { throw new RuntimeException("stub"); }
    public ParsedService getService(ComponentName component) { throw new RuntimeException("stub"); }
    public boolean componentExists(ComponentName componentName) { throw new RuntimeException("stub"); }
    public List<ResolveInfo> queryActivities(Computer computer, Intent intent,
            String resolvedType, long flags, int userId) { throw new RuntimeException("stub"); }
    public List<ResolveInfo> queryActivities(Computer computer, Intent intent,
            String resolvedType, long flags, List<ParsedActivity> activities,
            int userId) { throw new RuntimeException("stub"); }
    public ProviderInfo queryProvider(Computer computer, String authority, long flags,
            int userId) { throw new RuntimeException("stub"); }
    public List<ResolveInfo> queryProviders(Computer computer, Intent intent,
            String resolvedType, long flags, int userId) { throw new RuntimeException("stub"); }
    public List<ResolveInfo> queryProviders(Computer computer, Intent intent,
            String resolvedType, long flags, List<ParsedProvider> providers,
            int userId) { throw new RuntimeException("stub"); }
    public List<ProviderInfo> queryProviders(Computer computer, String processName,
            String metaDataKey, int uid, long flags, int userId) { throw new RuntimeException("stub"); }
    public List<ResolveInfo> queryReceivers(Computer computer, Intent intent,
            String resolvedType, long flags, int userId) { throw new RuntimeException("stub"); }
    public List<ResolveInfo> queryReceivers(Computer computer, Intent intent,
            String resolvedType, long flags, List<ParsedActivity> receivers,
            int userId) { throw new RuntimeException("stub"); }
    public List<ResolveInfo> queryServices(Computer computer, Intent intent,
            String resolvedType, long flags, int userId) { throw new RuntimeException("stub"); }
    public List<ResolveInfo> queryServices(Computer computer, Intent intent,
            String resolvedType, long flags, List<ParsedService> services,
            int userId) { throw new RuntimeException("stub"); }
    public void querySyncProviders(Computer computer, List<String> outNames,
            List<ProviderInfo> outInfo, boolean safeMode, int userId) { throw new RuntimeException("stub"); }
    public void dumpActivityResolvers(PrintWriter pw, DumpState dumpState,
            String packageName) { throw new RuntimeException("stub"); }
    public void dumpProviderResolvers(PrintWriter pw, DumpState dumpState,
            String packageName) { throw new RuntimeException("stub"); }
    public void dumpReceiverResolvers(PrintWriter pw, DumpState dumpState,
            String packageName) { throw new RuntimeException("stub"); }
    public void dumpServiceResolvers(PrintWriter pw, DumpState dumpState,
            String packageName) { throw new RuntimeException("stub"); }
    public void dumpContentProviders(Computer computer, PrintWriter pw,
            DumpState dumpState, String packageName) { throw new RuntimeException("stub"); }
    public void dumpServicePermissions(PrintWriter pw, DumpState dumpState) { throw new RuntimeException("stub"); }
}
