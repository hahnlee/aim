// Compile-only image API; checked by the device-services build node.
package com.android.internal.pm.pkg.component;
public class ParsedProviderImpl extends ParsedMainComponentImpl implements ParsedProvider {
    public ParsedProviderImpl() {}
    public String getAuthority() { throw new RuntimeException("stub"); }
    public boolean isSyncable() { throw new RuntimeException("stub"); }
}
