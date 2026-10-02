import java.io.ByteArrayOutputStream;
import java.io.ObjectInputStream;
import java.io.ObjectOutputStream;
import java.io.ByteArrayInputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.security.KeyFactory;
import java.security.KeyPairGenerator;
import java.security.PublicKey;
import java.security.spec.ECGenParameterSpec;
import java.security.spec.X509EncodedKeySpec;
import java.util.Arrays;

/** Original runtime oracle for native public-key serialization (#738). */
public final class PublicKeySerialization {
    private static String hex(byte[] bytes) {
        StringBuilder out = new StringBuilder();
        for (byte b : bytes) out.append(String.format("%02x", b & 255));
        return out.toString();
    }

    public static void main(String[] args) throws Exception {
        Path dir = Path.of(args[1]);
        String[] cases = {"RSA-1024", "RSA-2048", "EC-256", "EC-384", "EC-521", "DSA-1024"};
        for (String name : cases) {
            String algorithm = name.split("-")[0];
            int size = Integer.parseInt(name.split("-")[1]);
            Path input = dir.resolve(name + ".spki");
            if (args[0].equals("generate")) {
                KeyPairGenerator generator = KeyPairGenerator.getInstance(algorithm);
                if (algorithm.equals("EC")) {
                    generator.initialize(new ECGenParameterSpec("secp" + size + "r1"));
                } else {
                    generator.initialize(size);
                }
                Files.write(input, generator.generateKeyPair().getPublic().getEncoded());
                continue;
            }
            PublicKey key = KeyFactory.getInstance(algorithm).generatePublic(
                    new X509EncodedKeySpec(Files.readAllBytes(input)));
            if (args[0].equals("read")) {
                try (ObjectInputStream in = new ObjectInputStream(new ByteArrayInputStream(
                        Files.readAllBytes(dir.resolve(name + ".native"))))) {
                    PublicKey decoded = (PublicKey) in.readObject();
                    if (!Arrays.equals(key.getEncoded(), decoded.getEncoded())) {
                        throw new AssertionError("decoded key differs: " + name);
                    }
                }
            }
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            try (ObjectOutputStream out = new ObjectOutputStream(bytes)) {
                out.writeObject(key);
            }
            System.out.println(name + " " + key.getClass().getName() + " "
                    + key.hashCode() + " " + hex(bytes.toByteArray()));
        }
    }
}
