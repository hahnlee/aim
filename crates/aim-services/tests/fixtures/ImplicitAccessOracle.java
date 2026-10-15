package com.android.server.pm;

public final class ImplicitAccessOracle {
    public static void main(String[] args) {
        // Like SystemServer, own mutable memory for package-cache invalidation.
        var memory = com.android.internal.os.ApplicationSharedMemory.create();
        com.android.internal.os.ApplicationSharedMemory.setInstance(memory);
        try {
            AppsFilterImpl filter = new AppsFilterImpl(null, new String[0], false, null, null);
            int[][] grants = {{10002,10002,0}, {10002,10001,0}, {10002,10001,0},
                {10002,10001,1}, {10002,10001,1}, {20002,10001,1}, {20002,10001,0},
                {1010002,1010001,0}};
            for (int[] grant : grants) {
                boolean changed = filter.grantImplicitAccess(grant[0], grant[1], grant[2] != 0);
                System.out.println(changed + " "
                    + (filter.isImplicitlyQueryable(10002,10001) || filter.isRetainedImplicitlyQueryable(10002,10001))
                    + " " + filter.isImplicitlyQueryable(20002,10001)
                    + " " + (filter.isImplicitlyQueryable(1010002,1010001) || filter.isRetainedImplicitlyQueryable(1010002,1010001)));
            }
        } finally {
            memory.close();
        }
    }
}
