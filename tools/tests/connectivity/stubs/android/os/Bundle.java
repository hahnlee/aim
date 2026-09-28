package android.os;

import java.util.HashMap;
import java.util.Map;

/** Test-only Parcelable map used by ConnectivityManager.CallbackHandler messages. */
public final class Bundle {
    private final Map<String, Parcelable> values = new HashMap<String, Parcelable>();

    public void putParcelable(String key, Parcelable value) {
        values.put(key, value);
    }

    public <T extends Parcelable> T getParcelable(String key) {
        @SuppressWarnings("unchecked") T value = (T) values.get(key);
        return value;
    }
}
