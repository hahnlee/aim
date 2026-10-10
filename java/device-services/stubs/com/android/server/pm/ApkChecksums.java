// Compile-only pinned original API; verified by the device-services image linkage gate.
package com.android.server.pm;
public abstract class ApkChecksums {
    private ApkChecksums() { throw new RuntimeException("stub"); }
    static class Injector {
        interface Producer<T> { T produce(); }
        Injector(Producer<android.content.Context> context, Producer<android.os.Handler> handler,
                Producer<android.os.incremental.IncrementalManager> incremental,
                Producer<android.content.pm.PackageManagerInternal> packages) { throw new RuntimeException("stub"); }
    }
    public static java.security.cert.Certificate[] verifySignature(android.content.pm.Checksum[] checksums,
            byte[] signature) throws java.security.NoSuchAlgorithmException, java.io.IOException,
            java.security.SignatureException { throw new RuntimeException("stub"); }
    public static void getChecksums(java.util.List<android.util.Pair<String, java.io.File>> files,
            int optional, int required, String installer, java.security.cert.Certificate[] trusted,
            android.content.pm.IOnChecksumsReadyListener listener, Injector injector) { throw new RuntimeException("stub"); }
}
