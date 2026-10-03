import android.content.pm.IPackageManager;
import android.content.pm.PackageInfo;
import android.os.Parcel;
import android.os.ServiceManager;

/** SigningInfo from an actual original PMS reply, in its original Parcel. */
public final class SigningParcel {
    public static void main(String[] args) throws Exception {
        IPackageManager pm = IPackageManager.Stub.asInterface(ServiceManager.getService("package"));
        PackageInfo info = pm.getPackageInfo(args[0], 0x08000000L, 0);
        if (info == null || info.signingInfo == null) throw new AssertionError("no signing info");
        Parcel parcel = Parcel.obtain();
        try {
            info.signingInfo.writeToParcel(parcel, 0);
            StringBuilder text = new StringBuilder();
            for (byte b : parcel.marshall()) text.append(String.format("%02x", b & 255));
            System.out.println(text);
        } finally {
            parcel.recycle();
        }
    }
}
