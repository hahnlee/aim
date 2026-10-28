// Compile-only pinned image API; never packaged with the guest.
package android.security.keystore2;
public class AndroidKeyStoreProvider extends java.security.Provider {
 public AndroidKeyStoreProvider() { super("AndroidKeyStore", "1.0", "stub"); throw new RuntimeException("stub"); }
 public static void install() { throw new RuntimeException("stub"); }
}
