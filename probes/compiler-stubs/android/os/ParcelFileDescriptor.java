package android.os;

import java.io.Closeable;
import java.io.IOException;

/** Signature-only hidden API stub for compiler-side AOSP tests. */
public class ParcelFileDescriptor implements Parcelable, Closeable {
  public static ParcelFileDescriptor[] createPipe() throws IOException {
    return new ParcelFileDescriptor[] { new ParcelFileDescriptor(), new ParcelFileDescriptor() };
  }
  @Override public void close() throws IOException {}
  @Override public int describeContents() { return CONTENTS_FILE_DESCRIPTOR; }
  @Override public void writeToParcel(Parcel out, int flags) {}
}
