// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.content.pm;

public final class SigningDetails {
    public static final SigningDetails UNKNOWN = null;
    public SigningDetails(Signature[] signatures, int version, android.util.ArraySet<java.security.PublicKey> keys, Signature[] past) { throw new RuntimeException("stub"); }
    public boolean checkCapability(SigningDetails old, int flags) { throw new RuntimeException("stub"); }
    public boolean hasCommonAncestor(SigningDetails other) { throw new RuntimeException("stub"); }
    public boolean hasAncestor(SigningDetails old) { throw new RuntimeException("stub"); }
    public boolean hasAncestorOrSelf(SigningDetails old) { throw new RuntimeException("stub"); }
    public SigningDetails(Signature[] signatures, int signatureSchemeVersion) { throw new RuntimeException("stub"); }
    public int getSignatureSchemeVersion() { throw new RuntimeException("stub"); }
    public Signature[] getSignatures() { throw new RuntimeException("stub"); }
    public Signature[] getPastSigningCertificates() { throw new RuntimeException("stub"); }
    public android.util.ArraySet<java.security.PublicKey> getPublicKeys() { throw new RuntimeException("stub"); }
}
