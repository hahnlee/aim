// Compile-only image API; checked by the device-services build node.
package com.android.server.pm.verify.domain;
public class DomainVerificationPersistence {
    private DomainVerificationPersistence() { throw new RuntimeException("stub"); }
    public static ReadResult readFromXml(com.android.modules.utils.TypedXmlPullParser parser) throws java.io.IOException { throw new RuntimeException("stub"); }
    public static void writeToXml(com.android.modules.utils.TypedXmlSerializer serializer,
        com.android.server.pm.verify.domain.models.DomainVerificationStateMap attached,
        android.util.ArrayMap pending, android.util.ArrayMap restored, int user, java.util.function.Function signature) throws java.io.IOException { throw new RuntimeException("stub"); }
    public static class ReadResult {
        public final android.util.ArrayMap active = null;
        public final android.util.ArrayMap restored = null;
        public ReadResult(android.util.ArrayMap active, android.util.ArrayMap restored) { throw new RuntimeException("stub"); }
    }
}
