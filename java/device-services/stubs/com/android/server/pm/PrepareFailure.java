// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
final class PrepareFailure extends PackageManagerException {
    PrepareFailure(String message, Exception error) {
        super(0, message);
        throw new RuntimeException("stub");
    }
}
