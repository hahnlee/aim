// Compile-only image API; checked against the original image.
package com.android.server.pm.verify.domain;
public interface DomainVerificationManagerInternal {
    java.util.UUID generateNewId();
    java.util.UUID DISABLED_ID = new java.util.UUID(0, 0);
}
