package dev.aim.security;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.security.Key;
import java.security.KeyStoreException;
import java.security.KeyStoreSpi;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.security.UnrecoverableKeyException;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;
import java.util.Collections;
import java.util.Date;
import java.util.Enumeration;
import java.util.LinkedHashMap;
import java.util.HashMap;
import java.util.Map;

/** Read-only AndroidCAStore view backed by the macOS trust domains. */
public final class DarwinAndroidCAStore extends KeyStoreSpi {
    private static byte[][] cachedRoots;
    private static X509Certificate[] cachedIssuers;
    private static boolean installedSystemRoots;
    private final Map<String, Certificate> certificates = new LinkedHashMap<>();

    static synchronized void preload() {
        if (cachedRoots == null) cachedRoots = copyTrustedRoots();
        if (!installedSystemRoots) {
            installedSystemRoots = installSystemRoots(cachedRoots);
        }
    }

    static synchronized X509Certificate[] acceptedIssuers() {
        preload();
        if (cachedIssuers == null) {
            try {
                CertificateFactory factory = CertificateFactory.getInstance("X.509");
                byte[][] roots = cachedRoots;
                X509Certificate[] issuers =
                        new X509Certificate[roots == null ? 0 : roots.length];
                for (int index = 0; index < issuers.length; ++index) {
                    issuers[index] = (X509Certificate) factory.generateCertificate(
                            new java.io.ByteArrayInputStream(roots[index]));
                }
                cachedIssuers = issuers;
            } catch (CertificateException error) {
                android.util.Log.e(
                        "DarwinSecurity", "cannot decode macOS trust roots", error);
                cachedIssuers = new X509Certificate[0];
            }
        }
        return cachedIssuers.clone();
    }

    private static boolean installSystemRoots(byte[][] roots) {
        if (roots == null) return false;
        int installed = 0;
        try {
            CertificateFactory factory = CertificateFactory.getInstance("X.509");
            MessageDigest md5 = MessageDigest.getInstance("MD5");
            Map<String, Integer> collisions = new HashMap<>();
            for (byte[] encoded : roots) {
                Certificate certificate = factory.generateCertificate(
                        new java.io.ByteArrayInputStream(encoded));
                byte[] digest = md5.digest(
                        ((java.security.cert.X509Certificate) certificate)
                                .getSubjectX500Principal().getEncoded());
                char[] hex = "0123456789abcdef".toCharArray();
                char[] hash = new char[8];
                for (int index = 0; index < 4; ++index) {
                    int value = digest[3 - index] & 0xff;
                    hash[index * 2] = hex[value >>> 4];
                    hash[index * 2 + 1] = hex[value & 0xf];
                }
                String prefix = new String(hash);
                int suffix = collisions.containsKey(prefix) ? collisions.get(prefix) : 0;
                collisions.put(prefix, suffix + 1);
                if (writeSystemCertificate(prefix + "." + suffix, encoded)) ++installed;
            }
        } catch (CertificateException | NoSuchAlgorithmException error) {
            android.util.Log.e("DarwinSecurity", "cannot project macOS trust roots", error);
            return false;
        }
        android.util.Log.i("DarwinSecurity", "projected system roots=" + installed);
        return installed == roots.length;
    }

    @Override
    public void engineLoad(InputStream stream, char[] password)
            throws IOException, NoSuchAlgorithmException, CertificateException {
        certificates.clear();
        preload();
        byte[][] roots = cachedRoots;
        CertificateFactory factory = CertificateFactory.getInstance("X.509");
        for (int index = 0; roots != null && index < roots.length; ++index) {
            Certificate certificate = factory.generateCertificate(
                    new java.io.ByteArrayInputStream(roots[index]));
            // Chromium intentionally recognizes Android user roots by this prefix.
            certificates.put("user:darwin:" + index, certificate);
        }
        if (System.getenv("AIM_DEBUG_SECURITY") != null) {
            android.util.Log.i(
                    "DarwinSecurity",
                    "AndroidCAStore roots=" + certificates.size());
        }
    }

    @Override public Key engineGetKey(String alias, char[] password)
            throws NoSuchAlgorithmException, UnrecoverableKeyException { return null; }
    @Override public Certificate[] engineGetCertificateChain(String alias) { return null; }
    @Override public Certificate engineGetCertificate(String alias) {
        return certificates.get(alias);
    }
    @Override public Date engineGetCreationDate(String alias) {
        return certificates.containsKey(alias) ? new Date(0) : null;
    }
    @Override public void engineSetKeyEntry(String alias, Key key, char[] password,
            Certificate[] chain) throws KeyStoreException {
        throw new KeyStoreException("AndroidCAStore is read-only");
    }
    @Override public void engineSetKeyEntry(String alias, byte[] key, Certificate[] chain)
            throws KeyStoreException { throw new KeyStoreException("AndroidCAStore is read-only"); }
    @Override public void engineSetCertificateEntry(String alias, Certificate certificate)
            throws KeyStoreException { throw new KeyStoreException("AndroidCAStore is read-only"); }
    @Override public void engineDeleteEntry(String alias) throws KeyStoreException {
        throw new KeyStoreException("AndroidCAStore is read-only");
    }
    @Override public Enumeration<String> engineAliases() {
        return Collections.enumeration(certificates.keySet());
    }
    @Override public boolean engineContainsAlias(String alias) {
        return certificates.containsKey(alias);
    }
    @Override public int engineSize() { return certificates.size(); }
    @Override public boolean engineIsKeyEntry(String alias) { return false; }
    @Override public boolean engineIsCertificateEntry(String alias) {
        return certificates.containsKey(alias);
    }
    @Override public String engineGetCertificateAlias(Certificate certificate) {
        for (Map.Entry<String, Certificate> entry : certificates.entrySet()) {
            if (entry.getValue().equals(certificate)) return entry.getKey();
        }
        return null;
    }
    @Override public void engineStore(OutputStream stream, char[] password)
            throws IOException, NoSuchAlgorithmException, CertificateException {
        throw new IOException("AndroidCAStore is read-only");
    }

    private static native byte[][] copyTrustedRoots();
    private static native boolean writeSystemCertificate(String filename, byte[] encoded);
}
