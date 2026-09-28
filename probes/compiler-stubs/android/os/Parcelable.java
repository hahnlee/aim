package android.os;

/** Signature-only hidden API stub for compiler-side AOSP tests. */
public interface Parcelable {
  int CONTENTS_FILE_DESCRIPTOR = 1;
  int describeContents();
  void writeToParcel(Parcel out, int flags);

  interface Creator<T> {
    T createFromParcel(Parcel in);
    T[] newArray(int size);
  }
}
