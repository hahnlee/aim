package dev.darwinart.runtime.connectivity;

import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.URL;

/** Narrow transport for the pinned Android HTTP generate-204 validation slice. */
final class HttpNetworkProbeTransport implements NetworkProbeTransport {
    private static final String PROBE_URL =
            "http://connectivitycheck.gstatic.com/generate_204";
    private static final int TIMEOUT_MILLIS = 5000;

    @Override
    public Result probe() {
        HttpURLConnection connection = null;
        try {
            connection = (HttpURLConnection) new URL(PROBE_URL).openConnection();
            connection.setInstanceFollowRedirects(false);
            connection.setConnectTimeout(TIMEOUT_MILLIS);
            connection.setReadTimeout(TIMEOUT_MILLIS);
            connection.setUseCaches(false);
            connection.setRequestProperty("Connection", "close");
            return classifyResponse(connection.getResponseCode());
        } catch (IOException ignored) {
            return Result.FAILED;
        } finally {
            if (connection != null) connection.disconnect();
        }
    }

    static Result classifyResponse(int response) {
        if (response == HttpURLConnection.HTTP_NO_CONTENT) return Result.VALIDATED;
        if (response >= 300 && response < 400) return Result.CAPTIVE_PORTAL;
        return Result.FAILED;
    }
}
