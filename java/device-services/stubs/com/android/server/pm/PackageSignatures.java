// Compile-only image API; checked by the device-services build node.
package com.android.server.pm;
final class PackageSignatures {
    PackageSignatures() {}
    void readXml(com.android.modules.utils.TypedXmlPullParser parser, java.util.ArrayList<android.content.pm.Signature> certificates) throws java.io.IOException { throw new RuntimeException("stub"); }
    void writeXml(com.android.modules.utils.TypedXmlSerializer serializer, String tag,
        java.util.ArrayList<android.content.pm.Signature> certificates)
        throws java.io.IOException { throw new RuntimeException("stub"); }
    android.content.pm.SigningDetails mSigningDetails;
}
