package com.android.server.content;
import dev.aim.server.SystemProcessHostTest;
public class ContentService {
    private boolean constructed;
    public ContentService(android.content.Context context, boolean factory, SyncAccounts accounts) {
        if (SystemProcessHostTest.failSync) throw new IllegalStateException("sync construction failed");
        if (context == null || factory || accounts == null) throw new AssertionError();
        for (String owner : java.util.List.of("packageLocals", "attachSystem", "activityStats", "jobLocal", "policyLocals", "accountLocal"))
            if (!SystemProcessHostTest.events.contains(owner)) throw new AssertionError("missing " + owner);
    }
    public android.content.SyncAdapterType[] getSyncAdapterTypesAsUser(int user) {
        if (user != 0 || constructed) throw new AssertionError("second manager/cache construction");
        constructed = true;
        SystemProcessHostTest.events.add("syncConstructed");
        return new android.content.SyncAdapterType[0];
    }
}
