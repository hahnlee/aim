// Compile-only pinned image API.
package com.android.server.pm;
class ProcessLoggingHandler extends android.os.Handler {
    ProcessLoggingHandler() { throw new RuntimeException("stub"); }
    void logAppProcessStart(android.content.Context context, android.content.pm.PackageManagerInternal packages,
            String apkFile, String packageName, String processName, int uid, String seinfo, int pid) { throw new RuntimeException("stub"); }
    void invalidateBaseApkHash(String apkFile) { throw new RuntimeException("stub"); }
}
