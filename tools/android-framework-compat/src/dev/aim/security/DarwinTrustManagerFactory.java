package dev.aim.security;

import java.security.InvalidAlgorithmParameterException;
import java.security.KeyStore;
import java.security.KeyStoreException;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.Arrays;
import java.util.Collections;
import java.util.List;
import android.util.Log;
import javax.net.ssl.ManagerFactoryParameters;
import javax.net.ssl.TrustManager;
import javax.net.ssl.TrustManagerFactorySpi;
import javax.net.ssl.X509TrustManager;

/** Android TrustManagerFactory whose system trust decision is owned by macOS. */
public final class DarwinTrustManagerFactory extends TrustManagerFactorySpi {
    private static final boolean DEBUG = System.getenv("AIM_DEBUG_SECURITY") != null;
    private KeyStore explicitKeyStore;

    @Override
    protected void engineInit(KeyStore keyStore) throws KeyStoreException {
        explicitKeyStore = keyStore;
        Log.i("DarwinSecurity",
                "DARWIN security probe: TrustManagerFactory.engineInit keyStore="
                        + (keyStore == null ? "system" : keyStore.getType()));
    }

    @Override
    protected void engineInit(ManagerFactoryParameters parameters)
            throws InvalidAlgorithmParameterException {
        throw new InvalidAlgorithmParameterException("ManagerFactoryParameters unsupported");
    }

    @Override
    protected TrustManager[] engineGetTrustManagers() {
        Log.i("DarwinSecurity", "TrustManagerFactory.engineGetTrustManagers");
        if (DEBUG) {
            System.err.println(
                    "DARWIN security: TrustManagerFactory keyStore="
                            + (explicitKeyStore == null ? "system" : "explicit"));
        }
        return new TrustManager[] {new DarwinTrustManager(explicitKeyStore)};
    }

    /** Public hostname-aware shape consumed by Android's X509TrustManagerExtensions. */
    public static final class DarwinTrustManager implements X509TrustManager {
        private final KeyStore explicitKeyStore;

        DarwinTrustManager(KeyStore explicitKeyStore) {
            this.explicitKeyStore = explicitKeyStore;
            Log.i("DarwinSecurity", "DarwinTrustManager constructed");
        }

        @Override
        public void checkClientTrusted(X509Certificate[] chain, String authType)
                throws CertificateException {
            throw new CertificateException("client trust evaluation is unavailable");
        }

        @Override
        public void checkServerTrusted(X509Certificate[] chain, String authType)
                throws CertificateException {
            checkServerTrusted(chain, authType, null);
        }

        public List<X509Certificate> checkServerTrusted(
                X509Certificate[] chain, String authType, String host)
                throws CertificateException {
            Log.i("DarwinSecurity",
                    "DARWIN security probe: checkServerTrusted host=" + host
                            + " chain=" + (chain == null ? 0 : chain.length));
            if (DEBUG) {
                System.err.println(
                        "DARWIN security: verify host="
                                + host
                                + " authType="
                                + authType
                                + " chain="
                                + (chain == null ? 0 : chain.length));
            }
            if (chain == null || chain.length == 0) {
                throw new CertificateException("empty server certificate chain");
            }
            // A non-null store is Chromium's explicit test-certificate store.
            // Keep its anchors additive without replacing macOS system trust.
            if (explicitKeyStore != null) {
                try {
                    for (X509Certificate certificate : chain) {
                        if (explicitKeyStore.getCertificateAlias(certificate) != null) {
                            return Arrays.asList(chain.clone());
                        }
                    }
                } catch (KeyStoreException error) {
                    throw new CertificateException("explicit trust store unavailable", error);
                }
            }
            byte[][] encoded = new byte[chain.length][];
            for (int index = 0; index < chain.length; ++index) {
                encoded[index] = chain[index].getEncoded();
            }
            byte[][] trusted = verifyServerChain(encoded, host);
            if (trusted == null || trusted.length == 0) {
                throw new CertificateException("macOS rejected server certificate chain");
            }
            X509Certificate[] result = new X509Certificate[trusted.length];
            java.security.cert.CertificateFactory factory =
                    java.security.cert.CertificateFactory.getInstance("X.509");
            for (int index = 0; index < trusted.length; ++index) {
                result[index] = (X509Certificate) factory.generateCertificate(
                        new java.io.ByteArrayInputStream(trusted[index]));
            }
            return Collections.unmodifiableList(Arrays.asList(result));
        }

        public List<X509Certificate> checkServerTrusted(
                X509Certificate[] chain, byte[] ocspData, byte[] tlsSctData,
                String authType, String host) throws CertificateException {
            return checkServerTrusted(chain, authType, host);
        }

        public boolean isSameTrustConfiguration(String firstHost, String secondHost) {
            return true;
        }

        @Override
        public X509Certificate[] getAcceptedIssuers() {
            // UnityTls and other native Android TLS stacks seed their own
            // verifier from this standard X509TrustManager contract rather
            // than calling checkServerTrusted for each connection.
            X509Certificate[] issuers = DarwinAndroidCAStore.acceptedIssuers();
            if (DEBUG) {
                System.err.println(
                        "DARWIN security: accepted issuers=" + issuers.length);
            }
            return issuers;
        }

        private static native byte[][] verifyServerChain(byte[][] chain, String host);
    }
}
