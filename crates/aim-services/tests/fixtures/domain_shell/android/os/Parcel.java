package android.os;
/** Test boundary: the little-endian, aligned Android record emitted by native readShellState. */
public class Parcel {
 private java.nio.ByteBuffer b;
 public static Parcel obtain(){return new Parcel();}
 public void unmarshall(byte[] bytes,int off,int len){b=java.nio.ByteBuffer.wrap(bytes,off,len).order(java.nio.ByteOrder.LITTLE_ENDIAN);}
 public void setDataPosition(int pos){b.position(pos);} public int readInt(){return b.getInt();}
 public boolean readBoolean(){return readInt()!=0;} public void recycle(){}
 public String readString(){int n=readInt();if(n<0)return null;byte[] v=new byte[n*2];b.get(v);b.getShort();align();return new String(v,java.nio.charset.StandardCharsets.UTF_16LE);}
 private void align(){b.position((b.position()+3)&~3);}
 public byte[] createByteArray(){int n=readInt();byte[] v=new byte[n];b.get(v);align();return v;}
 public java.util.ArrayList<String> createStringArrayList(){int n=readInt();var r=new java.util.ArrayList<String>();for(int i=0;i<n;i++)r.add(readString());return r;}
 public void enforceNoDataAvail(){if(b.hasRemaining())throw new AssertionError("parcel tail");}
}
