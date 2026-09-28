package dev.aim.security;

import java.io.IOException;
import java.io.OutputStream;
import java.net.URL;
import javax.net.ssl.HttpsURLConnection;

/**
 * Opt-in transport probe for the detached runtime's normal HTTPS stack.
 *
 * This class is deliberately dormant unless AIM_DEBUG_HTTPS_URL is set.
 * It sends no application credentials and uses a daemon worker so provider
 * initialization is not made dependent on network availability.
 */
final class DarwinHttpsDiagnostic {
    private static final int TIMEOUT_MILLIS = 10_000;
    private static final byte[] EMPTY_JSON = new byte[] {'{', '}'};
    private static boolean started;

    private DarwinHttpsDiagnostic() {}

    static synchronized void startIfRequested() {
        if (started) return;
        String endpoint = System.getenv("AIM_DEBUG_HTTPS_URL");
        if (endpoint == null || endpoint.trim().isEmpty()) return;
        started = true;
        final String requestedEndpoint = endpoint.trim();
        Thread worker = new Thread(
                () -> run(requestedEndpoint), "darwin-https-diagnostic");
        worker.setDaemon(true);
        worker.start();
    }

    private static void run(String endpoint) {
        long startedAt = System.nanoTime();
        HttpsURLConnection connection = null;
        try {
            URL url = new URL(endpoint);
            if (!"https".equalsIgnoreCase(url.getProtocol())) {
                android.util.Log.w("DarwinHttpsDiagnostic", "refused non-HTTPS URL=" + endpoint);
                return;
            }
            android.util.Log.i("DarwinHttpsDiagnostic", "start URL=" + endpoint);
            String dataPath = System.getenv("AIM_APK_APP_DATA_GUEST_DIR");
            if (dataPath != null) {
                android.util.Log.i("DarwinHttpsDiagnostic", "app usable bytes="
                        + new java.io.File(dataPath).getUsableSpace());
            }
            connection = (HttpsURLConnection) url.openConnection();
            connection.setConnectTimeout(TIMEOUT_MILLIS);
            connection.setReadTimeout(TIMEOUT_MILLIS);
            connection.setRequestMethod("POST");
            connection.setDoOutput(true);
            connection.setUseCaches(false);
            connection.setRequestProperty("Content-Type", "application/json");
            connection.setFixedLengthStreamingMode(EMPTY_JSON.length);
            try (OutputStream output = connection.getOutputStream()) {
                output.write(EMPTY_JSON);
            }
            int responseCode = connection.getResponseCode();
            long elapsedMillis = (System.nanoTime() - startedAt) / 1_000_000L;
            android.util.Log.i("DarwinHttpsDiagnostic",
                    "DARWIN HTTPS probe response URL=" + endpoint
                            + " status=" + responseCode
                            + " message=" + connection.getResponseMessage()
                            + " elapsed_ms=" + elapsedMillis);
        } catch (IOException error) {
            long elapsedMillis = (System.nanoTime() - startedAt) / 1_000_000L;
            android.util.Log.e("DarwinHttpsDiagnostic",
                    "DARWIN HTTPS probe IOException URL=" + endpoint
                            + " elapsed_ms=" + elapsedMillis + " error=" + error, error);
            logCauses(error);
        } catch (Throwable error) {
            long elapsedMillis = (System.nanoTime() - startedAt) / 1_000_000L;
            android.util.Log.e("DarwinHttpsDiagnostic",
                    "DARWIN HTTPS probe failure URL=" + endpoint
                            + " elapsed_ms=" + elapsedMillis + " error=" + error, error);
            logCauses(error);
        } finally {
            if (connection != null) connection.disconnect();
        }
    }

    private static void logCauses(Throwable error) {
        // Log.getStackTraceString deliberately suppresses UnknownHostException.
        // This opt-in diagnostic needs its actual cause and callsite instead.
        for (int depth = 0; error != null && depth < 5; depth++, error = error.getCause()) {
            android.util.Log.e("DarwinHttpsDiagnostic", "cause[" + depth + "] " + error);
            StackTraceElement[] trace = error.getStackTrace();
            for (int i = 0; i < trace.length && i < 20; i++) {
                android.util.Log.e("DarwinHttpsDiagnostic", "  at " + trace[i]);
            }
        }
    }
}
