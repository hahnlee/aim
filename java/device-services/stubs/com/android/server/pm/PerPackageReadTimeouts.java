package com.android.server.pm;
class PerPackageReadTimeouts {
 PerPackageReadTimeouts(String name,byte[] certificate,VersionCodes versions,Timeouts timeouts){throw new RuntimeException("stub");}
    public String packageName; public byte[] sha256certificate;
    public VersionCodes versionCodes;public Timeouts timeouts;
    static java.util.List<PerPackageReadTimeouts> parseDigestersList(String defaults,String digesters){throw new RuntimeException("stub");}
    static class VersionCodes {VersionCodes(long min,long max){throw new RuntimeException("stub");}public long minVersionCode,maxVersionCode;}
    static class Timeouts {Timeouts(long min,long pending,long max){throw new RuntimeException("stub");}public long minTimeUs,minPendingTimeUs,maxPendingTimeUs;}
}
