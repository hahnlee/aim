// Host-only Android boundary double; production policy is compiled unchanged.
package android.util.proto;
import java.util.*;
/** Record the real writer's field IDs/values; Android's encoding is linkage-checked separately. */
public class ProtoOutputStream extends ProtoStream {
    public static final List<Map<Long,Object>> records=new ArrayList<>();
    public static boolean delivered;
    private Map<Long,Object> current;
    public ProtoOutputStream(java.io.OutputStream output) {}
    public long start(long field) {
        if(field!=(FIELD_TYPE_MESSAGE|FIELD_COUNT_REPEATED|1)) throw new AssertionError(field);
        current=new HashMap<>(); records.add(current); return records.size();
    }
    public void end(long token) {}
    public void write(long field,int value) { current.put(field,value); }
    public void write(long field,long value) { current.put(field,value); }
    public void write(long field,String value) { current.put(field,value); }
    public void write(long field,byte[] value) { current.put(field,value.clone()); }
    public void flush() { if(!delivered) throw new AssertionError("flushed before statsd got FD"); }
}
