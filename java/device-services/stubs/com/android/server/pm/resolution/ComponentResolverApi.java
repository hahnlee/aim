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
public interface ComponentResolverApi {

    ParsedActivity getActivity(ComponentName component);

    ParsedProvider getProvider(ComponentName component);

    ParsedActivity getReceiver(ComponentName component);

    ParsedService getService(ComponentName component);


    boolean componentExists(ComponentName componentName);

    List<ResolveInfo> queryActivities(Computer computer, Intent intent,
            String resolvedType, long flags, int userId);

    List<ResolveInfo> queryActivities(Computer computer, Intent intent,
            String resolvedType, long flags, List<ParsedActivity> activities,
            int userId);

    ProviderInfo queryProvider(Computer computer, String authority, long flags,
            int userId);

    List<ResolveInfo> queryProviders(Computer computer, Intent intent,
            String resolvedType, long flags, int userId);

    List<ResolveInfo> queryProviders(Computer computer, Intent intent,
            String resolvedType, long flags, List<ParsedProvider> providers,
            int userId);

    List<ProviderInfo> queryProviders(Computer computer, String processName,
            String metaDataKey, int uid, long flags, int userId);

    List<ResolveInfo> queryReceivers(Computer computer, Intent intent,
            String resolvedType, long flags, int userId);

    List<ResolveInfo> queryReceivers(Computer computer, Intent intent,
            String resolvedType, long flags, List<ParsedActivity> receivers,
            int userId);

    List<ResolveInfo> queryServices(Computer computer, Intent intent,
            String resolvedType, long flags, int userId);

    List<ResolveInfo> queryServices(Computer computer, Intent intent,
            String resolvedType, long flags, List<ParsedService> services,
            int userId);

    void querySyncProviders(Computer computer, List<String> outNames,
            List<ProviderInfo> outInfo, boolean safeMode, int userId);

    void dumpActivityResolvers(PrintWriter pw, DumpState dumpState,
            String packageName);

    void dumpProviderResolvers(PrintWriter pw, DumpState dumpState,
            String packageName);

    void dumpReceiverResolvers(PrintWriter pw, DumpState dumpState,
            String packageName);

    void dumpServiceResolvers(PrintWriter pw, DumpState dumpState,
            String packageName);

    void dumpContentProviders(Computer computer, PrintWriter pw,
            DumpState dumpState, String packageName);

    void dumpServicePermissions(PrintWriter pw, DumpState dumpState);
}
