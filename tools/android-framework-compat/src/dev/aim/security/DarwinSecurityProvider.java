package dev.aim.security;

import java.security.Provider;

/** Boot-class-path JCA provider for a detached Darwin Android process. */
public final class DarwinSecurityProvider extends Provider {
    public DarwinSecurityProvider() {
        // Android exposes this as a distinct provider name; keeping that name
        // is required for APKs that explicitly request AndroidKeyStore.
        super("AndroidKeyStore", 1.0, "aim AndroidKeyStore provider");
        put("SecureRandom.SHA1PRNG", DarwinSecureRandom.class.getName());
        put("SecureRandom.NativePRNG", DarwinSecureRandom.class.getName());
        put("TrustManagerFactory.PKIX", DarwinTrustManagerFactory.class.getName());
        put("Alg.Alias.TrustManagerFactory.X509", "PKIX");
        put("Alg.Alias.TrustManagerFactory.SunX509", "PKIX");
        put("KeyStore.AndroidCAStore", DarwinAndroidCAStore.class.getName());
        put("KeyStore.AndroidKeyStore", DarwinAndroidKeyStore.class.getName());
        put("KeyGenerator.HmacSHA256", DarwinAndroidKeyStore.HmacKeyGenerator.class.getName());
        put("Mac.HmacSHA256", DarwinAndroidKeyStore.HmacMac.class.getName());
        // Conscrypt's default KeyManagerFactory asks for the Android BKS
        // trust-store type when no explicit KeyStore is supplied.  Android
        // ships that type as part of its platform provider; map it to the
        // same macOS-backed, read-only CA view so ordinary OkHttp startup
        // follows the host trust roots without requiring an APK-side store.
        put("KeyStore.BKS", DarwinAndroidCAStore.class.getName());
        // Keystore users must remain usable even when the optional native CA
        // bridge is not loaded (for example in a host-side acceptance JVM).
        // Android treats each provider service independently; do not make
        // AndroidKeyStore construction fail because AndroidCAStore is absent.
        try {
            DarwinAndroidCAStore.preload();
        } catch (LinkageError ignored) {
            if (System.getenv("AIM_DEBUG_SECURITY") != null) {
                System.err.println("DARWIN security: AndroidCAStore unavailable");
            }
        }
        DarwinHttpsDiagnostic.startIfRequested();
        if (System.getenv("AIM_DEBUG_SECURITY") != null) {
            System.err.println("DARWIN security: provider initialized");
        }
    }
}
