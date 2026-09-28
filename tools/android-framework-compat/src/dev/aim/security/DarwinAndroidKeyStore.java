package dev.aim.security;

import java.io.IOException;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.OutputStream;
import java.security.Key;
import java.security.KeyStore;
import java.security.KeyStoreException;
import java.security.KeyStoreSpi;
import java.security.InvalidAlgorithmParameterException;
import java.security.InvalidKeyException;
import java.security.SecureRandom;
import java.security.NoSuchAlgorithmException;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.util.Collections;
import java.util.Date;
import java.util.Enumeration;
import java.util.Map;
import java.util.HashMap;
import java.util.ArrayList;
import java.util.List;
import javax.crypto.MacSpi;
import javax.crypto.KeyGeneratorSpi;
import javax.crypto.SecretKey;
import javax.crypto.ShortBufferException;
import java.security.spec.AlgorithmParameterSpec;

/** Explicit AndroidKeyStore boundary; no host key material is exposed. */
public final class DarwinAndroidKeyStore extends KeyStoreSpi {
    private static final Map<String, SecretKey> KEYS = new HashMap<>();
    private static boolean loaded;
    private static File backingFile() {
        String root = System.getenv("AIM_APK_APP_DATA_DIR");
        if (root == null || root.length() == 0) return null;
        return new File(new File(root, "keystore"), "android-keystore-hmac-v1");
    }
    private static synchronized void loadPersistent() {
        if (loaded) return;
        loaded = true;
        File file = backingFile();
        if (file == null || !file.isFile()) return;
        try {
            FileInputStream input = new FileInputStream(file);
            byte[] bytes = new byte[(int) Math.min(file.length(), 1 << 20)];
            int count = input.read(bytes);
            input.close();
            if (count <= 0) return;
            String[] records = new String(bytes, 0, count, "UTF-8").split("\\n");
            for (String record : records) {
                int separator = record.indexOf('=');
                if (separator <= 0) continue;
                byte[] material = decodeHex(record.substring(separator + 1));
                if (material != null && material.length != 0) {
                    KEYS.put(record.substring(0, separator), new HmacKey(material));
                }
            }
        } catch (Exception ignored) { }
    }
    private static synchronized void persist() {
        File file = backingFile();
        if (file == null) return;
        File parent = file.getParentFile();
        if (parent == null) return;
        parent.mkdirs();
        File temporary = new File(parent, file.getName() + ".tmp-" + Long.toHexString(System.nanoTime()));
        try {
            FileOutputStream output = new FileOutputStream(temporary);
            for (Map.Entry<String, SecretKey> entry : KEYS.entrySet()) {
                SecretKey value = entry.getValue();
                if (value instanceof HmacKey) {
                    output.write(entry.getKey().getBytes("UTF-8"));
                    output.write('=');
                    output.write(encodeHex(((HmacKey) value).material()).getBytes("UTF-8"));
                    output.write('\n');
                }
            }
            output.close();
            if (!temporary.renameTo(file)) temporary.delete();
        } catch (Exception ignored) {
            temporary.delete();
        }
    }
    private static String encodeHex(byte[] bytes) {
        char[] digits = "0123456789abcdef".toCharArray();
        char[] result = new char[bytes.length * 2];
        for (int i = 0; i < bytes.length; i++) {
            int value = bytes[i] & 0xff;
            result[i * 2] = digits[value >>> 4];
            result[i * 2 + 1] = digits[value & 15];
        }
        return new String(result);
    }
    private static byte[] decodeHex(String value) {
        if ((value.length() & 1) != 0) return null;
        byte[] result = new byte[value.length() / 2];
        for (int i = 0; i < result.length; i++) {
            int high = Character.digit(value.charAt(i * 2), 16);
            int low = Character.digit(value.charAt(i * 2 + 1), 16);
            if (high < 0 || low < 0) return null;
            result[i] = (byte) ((high << 4) | low);
        }
        return result;
    }
    static synchronized SecretKey key(String alias) { loadPersistent(); return KEYS.get(alias); }
    static synchronized void put(String alias, SecretKey key) { loadPersistent(); KEYS.put(alias, key); persist(); }
    @Override public Key engineGetKey(String alias, char[] password) { return key(alias); }
    @Override public Certificate[] engineGetCertificateChain(String alias) { return null; }
    @Override public Certificate engineGetCertificate(String alias) { return null; }
    @Override public Date engineGetCreationDate(String alias) { return null; }
    @Override public void engineSetKeyEntry(String alias, Key key, char[] password,
            Certificate[] chain) throws KeyStoreException {
        throw new KeyStoreException("AndroidKeyStore key import is unsupported on Darwin");
    }
    @Override public void engineSetKeyEntry(String alias, byte[] key,
            Certificate[] chain) throws KeyStoreException {
        throw new KeyStoreException("AndroidKeyStore key import is unsupported on Darwin");
    }
    @Override public void engineSetCertificateEntry(String alias, Certificate cert)
            throws KeyStoreException {
        throw new KeyStoreException("AndroidKeyStore certificate import is unsupported on Darwin");
    }
    @Override public synchronized void engineDeleteEntry(String alias) throws KeyStoreException {
        loadPersistent();
        KEYS.remove(alias);
        persist();
    }
    @Override public synchronized Enumeration<String> engineAliases() {
        loadPersistent();
        return Collections.enumeration(new java.util.ArrayList<>(KEYS.keySet()));
    }
    @Override public synchronized boolean engineContainsAlias(String alias) {
        loadPersistent();
        return KEYS.containsKey(alias);
    }
    @Override public synchronized int engineSize() { loadPersistent(); return KEYS.size(); }
    @Override public synchronized boolean engineIsKeyEntry(String alias) {
        loadPersistent();
        return KEYS.containsKey(alias);
    }
    @Override public boolean engineIsCertificateEntry(String alias) { return false; }
    @Override public String engineGetCertificateAlias(Certificate cert) { return null; }
    @Override public void engineStore(OutputStream stream, char[] password)
            throws IOException, NoSuchAlgorithmException, CertificateException {
        if (stream != null) throw new IOException("AndroidKeyStore is not exportable");
    }
    @Override public void engineLoad(InputStream stream, char[] password)
            throws IOException, NoSuchAlgorithmException, CertificateException {
        if (stream != null) throw new IOException("AndroidKeyStore is not importable");
        loadPersistent();
    }
    @Override public synchronized KeyStore.Entry engineGetEntry(String alias,
            KeyStore.ProtectionParameter protection) {
        loadPersistent();
        SecretKey value = KEYS.get(alias);
        return value == null ? null : new KeyStore.SecretKeyEntry(value);
    }
    @Override public boolean engineEntryInstanceOf(String alias,
            Class<? extends KeyStore.Entry> entryClass) { return false; }

    public static final class HmacKey implements SecretKey {
        private final byte[] material;
        HmacKey(byte[] material) { this.material = material.clone(); }
        byte[] material() { return material.clone(); }
        @Override public String getAlgorithm() { return "HmacSHA256"; }
        @Override public String getFormat() { return null; }
        @Override public byte[] getEncoded() { return null; }
    }

    public static final class HmacKeyGenerator extends KeyGeneratorSpi {
        private SecureRandom random = new SecureRandom();
        private String alias = "key2";
        @Override protected void engineInit(SecureRandom random) {
            if (random != null) this.random = random;
        }
        @Override protected void engineInit(AlgorithmParameterSpec params, SecureRandom random)
                throws InvalidAlgorithmParameterException {
            if (random != null) this.random = random;
            if (params != null) {
                try {
                    java.lang.reflect.Method getAlias = params.getClass().getMethod("getKeystoreAlias");
                    Object value = getAlias.invoke(params);
                    if (value instanceof String) alias = (String) value;
                } catch (ReflectiveOperationException ignored) { }
            }
        }
        @Override protected void engineInit(int keysize, SecureRandom random) {
            if (random != null) this.random = random;
        }
        @Override protected SecretKey engineGenerateKey() {
            byte[] material = new byte[32];
            random.nextBytes(material);
            HmacKey key = new HmacKey(material);
            put(alias, key);
            return key;
        }
    }

    public static final class HmacMac extends MacSpi {
        private byte[] key;
        private final java.io.ByteArrayOutputStream input = new java.io.ByteArrayOutputStream();
        @Override protected int engineGetMacLength() { return 32; }
        @Override protected void engineInit(Key key, AlgorithmParameterSpec params)
                throws InvalidKeyException {
            if (key instanceof HmacKey) this.key = ((HmacKey) key).material();
            else if (key instanceof SecretKey && key.getEncoded() != null) {
                this.key = key.getEncoded().clone();
            } else throw new InvalidKeyException("not a usable HMAC key");
            engineReset();
        }
        @Override protected void engineUpdate(byte inputByte) { input.write(inputByte); }
        @Override protected void engineUpdate(byte[] bytes, int offset, int len) {
            input.write(bytes, offset, len);
        }
        @Override protected byte[] engineDoFinal() {
            if (key == null) throw new IllegalStateException("HMAC key is not initialized");
            byte[] block = new byte[64];
            byte[] material = key;
            if (material.length > block.length) material = sha256(material);
            System.arraycopy(material, 0, block, 0, material.length);
            byte[] outer = block.clone(), inner = block.clone();
            for (int i = 0; i < block.length; i++) { outer[i] ^= 0x5c; inner[i] ^= 0x36; }
            byte[] message = input.toByteArray();
            byte[] innerHash = sha256(concat(inner, message));
            byte[] result = sha256(concat(outer, innerHash));
            engineReset();
            return result;
        }
        @Override protected void engineReset() { input.reset(); }
        private static byte[] sha256(byte[] bytes) {
            try { return java.security.MessageDigest.getInstance("SHA-256").digest(bytes); }
            catch (java.security.NoSuchAlgorithmException e) { throw new AssertionError(e); }
        }
        private static byte[] concat(byte[] a, byte[] b) {
            byte[] out = new byte[a.length + b.length];
            System.arraycopy(a, 0, out, 0, a.length);
            System.arraycopy(b, 0, out, a.length, b.length);
            return out;
        }
    }
}
