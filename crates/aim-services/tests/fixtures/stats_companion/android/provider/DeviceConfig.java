// Host-only Android boundary double; production policy is compiled unchanged.
package android.provider;
import java.util.*;
import java.util.concurrent.Executor;
public final class DeviceConfig {
    public static final String NAMESPACE_STATSD_JAVA = "statsd_java";
    public interface OnPropertiesChangedListener { void onPropertiesChanged(Properties properties); }
    public static final Map<OnPropertiesChangedListener,Executor> listeners = new LinkedHashMap<>();
    public static final Map<String,String> values = new LinkedHashMap<>();
    public static void addOnPropertiesChangedListener(String namespace,Executor executor,OnPropertiesChangedListener listener) {
        if(!namespace.equals(NAMESPACE_STATSD_JAVA)) throw new AssertionError(namespace);
        listeners.put(listener,executor);
    }
    public static void removeOnPropertiesChangedListener(OnPropertiesChangedListener listener) {
        if(listeners.remove(listener)==null) throw new AssertionError("listener double remove");
    }
    public static Properties getProperties(String namespace,String... names) {
        if(!namespace.equals(NAMESPACE_STATSD_JAVA) || names.length!=0) throw new AssertionError("snapshot namespace");
        return new Properties(namespace,values);
    }
    public static void change(Map<String,String> changes) {
        for(var e:changes.entrySet()) { if(e.getValue()==null) values.remove(e.getKey());else values.put(e.getKey(),e.getValue()); }
        Properties properties=new Properties(NAMESPACE_STATSD_JAVA,changes);
        for(var e:List.copyOf(listeners.entrySet())) e.getValue().execute(()->e.getKey().onPropertiesChanged(properties));
    }
    public static class Properties {
        final Map<String,String> values;
        public Properties(String namespace,Map<String,String> values) { this.values=new LinkedHashMap<>(values); }
        public Set<String> getKeyset() { return values.keySet(); }
        public String getString(String key,String fallback) { return values.getOrDefault(key,fallback); }
    }
}
