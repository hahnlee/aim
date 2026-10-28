package dev.aim.server;

/** RegisteredServicesCache discovery, with the original caller's package visibility. */
interface ISyncAdapterQueries {
    byte[] getSyncAdapterTypes(int callingUid, int userId);
}
