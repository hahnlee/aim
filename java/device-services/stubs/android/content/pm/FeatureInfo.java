// Compile-only original SDK API; never packaged into the image.
package android.content.pm;
public class FeatureInfo implements android.os.Parcelable {
 public String name; public int version; public int reqGlEsVersion; public int flags;
 public static final android.os.Parcelable.Creator<FeatureInfo> CREATOR=null;
 public FeatureInfo(){throw new RuntimeException("stub");}
 public int describeContents(){throw new RuntimeException("stub");}
 public void writeToParcel(android.os.Parcel out,int flags){throw new RuntimeException("stub");}
}
