package android.os;
import android.content.UriRelativeFilterGroupParcel;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.Set;
/** Android Bundle boundary: fail if the adapter uses anything but its typed getter. */
public class Bundle {
    public final LinkedHashMap<String, Object> values = new LinkedHashMap<>();
    public int getters;
    public boolean isEmpty() { return values.isEmpty(); }
    public Set<String> keySet() { return values.keySet(); }
    @SuppressWarnings("unchecked")
    public <T> ArrayList<T> getParcelableArrayList(String key, Class<T> type) {
        if (type != UriRelativeFilterGroupParcel.class) throw new AssertionError(type);
        getters++;
        Object value = values.get(key);
        if (!(value instanceof ArrayList<?> list)) return null;
        for (Object item : list) if (item != null && !type.isInstance(item)) return null;
        return (ArrayList<T>) list;
    }
}
