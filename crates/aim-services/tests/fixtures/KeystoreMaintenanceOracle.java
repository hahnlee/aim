public final class KeystoreMaintenanceOracle {
    public static void main(String[] args) {
        // Empty namespaces in this disposable boot; no existing user data.
        for (long namespace : new long[] {19001, 1019001}) {
            int result = android.security.AndroidKeyStoreMaintenance.clearNamespace(0, namespace);
            System.out.println(namespace + " " + result);
        }
    }
}
