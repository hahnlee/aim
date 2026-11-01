package android.os;

public class Bundle {
    private final java.util.Map<String,Object> values=new java.util.TreeMap<>();
    public void putInt(String key,int value){values.put(key,value);}
    public void putBoolean(String key,boolean value){values.put(key,value);}
    public void writeToParcel(Parcel out){
        Parcel body=Parcel.obtain();body.writeInt(values.size());
        values.forEach((key,value)->{
            body.writeString(key);
            body.writeInt(value instanceof Boolean?9:1);
            body.writeInt(value instanceof Boolean?((Boolean)value?1:0):(Integer)value);
        });
        byte[] data=body.marshall();out.writeInt(data.length);out.writeInt(0x4c444e42);
        for(int i=0;i<data.length;i+=4)out.writeInt(java.nio.ByteBuffer.wrap(data,i,4)
                .order(java.nio.ByteOrder.LITTLE_ENDIAN).getInt());
        out.writeInt(0); // Android 16 Bundle's hasIntent flag
    }
    public void putAll(Bundle other) { throw new AssertionError("unexercised extras boundary"); }
}
