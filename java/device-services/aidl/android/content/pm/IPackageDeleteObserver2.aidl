// Original android-16.0.0_r1 Binder callback (AOSP, Apache 2.0).
package android.content.pm;
import android.content.Intent;
oneway interface IPackageDeleteObserver2 {
    void onUserActionRequired(in Intent intent);
    void onPackageDeleted(String packageName, int returnCode, String msg);
}
