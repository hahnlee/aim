// Compile-only pinned image API, checked by the device-services build.
package com.android.server.pm;
public abstract class VerificationUtils {
    private VerificationUtils(){throw new RuntimeException("compile-only");}
    public static long getDefaultVerificationTimeout(android.content.Context context){throw new RuntimeException("stub");}
}
