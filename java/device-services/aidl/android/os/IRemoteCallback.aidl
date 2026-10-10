// Original Binder contract at android-16.0.0_r1 (AOSP, Apache 2.0).
package android.os;
import android.os.Bundle;
oneway interface IRemoteCallback {
    void sendResult(in Bundle data);
}
