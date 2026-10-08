// Original android-16.0.0_r1 Binder callback (AOSP, Apache 2.0).
package android.content.pm;
import android.os.Bundle;
oneway interface IPackageMoveObserver {
    void onCreated(int moveId, in Bundle extras);
    void onStatusChanged(int moveId, int status, long estMillis);
}
