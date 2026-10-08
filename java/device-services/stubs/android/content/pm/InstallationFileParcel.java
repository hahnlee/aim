package android.content.pm;
public class InstallationFileParcel implements android.os.Parcelable {
    public int location;public String name;public long size;public byte[] metadata,signature;
    public static final android.os.Parcelable.Creator<InstallationFileParcel> CREATOR=null;
    public void writeToParcel(android.os.Parcel p,int flags){throw new RuntimeException("stub");}
    public int describeContents(){throw new RuntimeException("stub");}
}
