// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
public class PackageManagerException extends Exception {
    public final int error;
    public PackageManagerException(int error, String message) { throw new RuntimeException("stub"); }
}
