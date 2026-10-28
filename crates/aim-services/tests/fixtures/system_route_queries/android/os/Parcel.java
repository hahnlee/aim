package android.os;
import java.nio.charset.StandardCharsets;
public class Parcel {
 private Object value;
 public static int live;
 public static Parcel obtain(){live++;return new Parcel();}
 public void writeTypedList(java.util.List<?> v){value=v;}
 public void writeTypedObject(Object v,int flags){value=v;}
 public byte[] marshall(){return String.valueOf(value).getBytes(StandardCharsets.UTF_8);}
 public void recycle(){live--;}
}
