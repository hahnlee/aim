// Compile-only API checked against the pinned image.
package com.android.server.pm.verify.domain;
public class DomainVerificationCollector {
    public DomainVerificationCollector(com.android.server.compat.PlatformCompat compat, com.android.server.SystemConfig config) { throw new RuntimeException("stub"); }
    public android.util.ArraySet<String> collectAllWebDomains(com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
    public android.util.ArraySet<String> collectValidAutoVerifyDomains(com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
    public android.util.ArraySet<String> collectInvalidAutoVerifyDomains(com.android.server.pm.pkg.AndroidPackage pkg) { throw new RuntimeException("stub"); }
}
