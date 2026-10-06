// Compile-only image API; checked against the original image.
package com.android.server.pm.verify.domain;
public interface DomainVerificationManagerInternal {
    java.util.UUID generateNewId();
    com.android.server.pm.verify.domain.proxy.DomainVerificationProxy getProxy();
    interface Connection extends DomainVerificationEnforcer.Callback {
        void scheduleWriteSettings();
        int getCallingUid();
        int getCallingUserId();
        int[] getAllUserIds();
        com.android.server.pm.Computer snapshot();
    }
    java.util.UUID DISABLED_ID = new java.util.UUID(0, 0);
}
