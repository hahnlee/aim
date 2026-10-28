// Host-only Android boundary double; production policy is compiled unchanged.
package android.content.pm;
public class SigningInfo {
    public Signature[] signers;
    public Signature[] getApkContentsSigners() { return signers; }
}
