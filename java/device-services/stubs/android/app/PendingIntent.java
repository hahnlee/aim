// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

import android.content.Context;
import android.content.Intent;
import android.os.Bundle;
import android.os.UserHandle;

public final class PendingIntent implements android.os.Parcelable {
    public static final int FLAG_IMMUTABLE = 0x04000000;
    public static PendingIntent getActivity(Context context, int requestCode, Intent intent, int flags) { throw new RuntimeException("stub"); }
    public String getCreatorPackage() { throw new RuntimeException("stub"); }
    public int describeContents(){throw new RuntimeException("stub");}
    public void writeToParcel(android.os.Parcel p,int flags){throw new RuntimeException("stub");}
    public static final int FLAG_MUTABLE = 1 << 25;

    public PendingIntent(android.content.IIntentSender target) { throw new RuntimeException("stub"); }
    public static PendingIntent getActivityAsUser(Context context, int requestCode, Intent intent, int flags, Bundle options, UserHandle user) { throw new RuntimeException("stub"); }
 public static final android.os.Parcelable.Creator<PendingIntent> CREATOR=null;
}
