package android.os;

/** Signature-only hidden API stub for compiler-side AOSP tests. */
public class Parcel {
  public static Parcel obtain() { return null; }
  public void recycle() {}
  public void setDataPosition(int position) {}
  public boolean hasFileDescriptors() { return false; }
  public void writeInt(int value) {}
  public int readInt() { return 0; }
  public void writeString(String value) {}
  public String readString() { return null; }
  public void writeStrongBinder(IBinder value) {}
  public IBinder readStrongBinder() { return null; }
  public void writeParcelable(Parcelable value, int flags) {}
  public <T extends Parcelable> T readParcelable(ClassLoader loader) { return null; }
}
