import dev.aim.security.DarwinSecurityProvider;
import java.nio.charset.StandardCharsets;
import java.security.KeyStore;
import java.security.Security;
import javax.crypto.KeyGenerator;
import javax.crypto.Mac;
import javax.crypto.SecretKey;

public final class AndroidKeyStoreCrossProcess {
    private static final String ALIAS = "key2";
    private static final byte[] MESSAGE = "aim-cross-process".getBytes(StandardCharsets.UTF_8);
    public static void main(String[] args) throws Exception {
        Security.insertProviderAt(new DarwinSecurityProvider(), 1);
        KeyStore store = KeyStore.getInstance("AndroidKeyStore"); store.load(null, null);
        if ("writer".equals(args[0])) {
            if (store.containsAlias(ALIAS)) store.deleteEntry(ALIAS);
            SecretKey key = KeyGenerator.getInstance("HmacSHA256", "AndroidKeyStore").generateKey();
            if (key.getEncoded() != null) throw new AssertionError("keystore key exposed");
            System.out.println("writer alias=" + store.containsAlias(ALIAS) + " mac=" + hex(mac(key))); return;
        }
        if (!store.containsAlias(ALIAS)) throw new AssertionError("alias missing across process");
        SecretKey key = (SecretKey) store.getKey(ALIAS, null);
        if (key == null || key.getEncoded() != null) throw new AssertionError("opaque key contract failed");
        System.out.println("reader mac=" + hex(mac(key))); store.deleteEntry(ALIAS);
        if (store.containsAlias(ALIAS)) throw new AssertionError("delete failed");
    }
    private static byte[] mac(SecretKey key) throws Exception { Mac m = Mac.getInstance("HmacSHA256", "AndroidKeyStore"); m.init(key); return m.doFinal(MESSAGE); }
    private static String hex(byte[] value) { StringBuilder r = new StringBuilder(); for (byte b : value) r.append(String.format("%02x", b & 255)); return r.toString(); }
}
