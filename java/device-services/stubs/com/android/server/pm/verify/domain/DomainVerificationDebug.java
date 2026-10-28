// Compile-only android-16.0.0_r1 API; never packaged.
package com.android.server.pm.verify.domain;
public class DomainVerificationDebug {
    DomainVerificationDebug(DomainVerificationCollector collector) { throw new RuntimeException("stub"); }
    boolean printState(android.util.IndentingPrintWriter writer, com.android.server.pm.verify.domain.models.DomainVerificationPkgState state, com.android.server.pm.pkg.AndroidPackage pkg, android.util.ArrayMap<String, Integer> map, boolean header) { throw new RuntimeException("stub"); }
    void printState(android.util.IndentingPrintWriter writer, com.android.server.pm.verify.domain.models.DomainVerificationPkgState state, com.android.server.pm.pkg.AndroidPackage pkg, Integer user, android.util.ArraySet<String> set, boolean header) { throw new RuntimeException("stub"); }
    public void printOwners(android.util.IndentingPrintWriter writer, String domain, android.util.SparseArray<android.util.SparseArray<java.util.List<String>>> users) { throw new RuntimeException("stub"); }
}
