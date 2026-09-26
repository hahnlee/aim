package android.window;

import android.os.Parcel;
import android.os.Parcelable;

public final class OnBackInvokedCallbackInfo {
    public static final Parcelable.Creator<OnBackInvokedCallbackInfo> CREATOR =
            new Parcelable.Creator<OnBackInvokedCallbackInfo>() {
                @Override public OnBackInvokedCallbackInfo createFromParcel(Parcel source) {
                    return new OnBackInvokedCallbackInfo();
                }
                @Override public OnBackInvokedCallbackInfo[] newArray(int size) {
                    return new OnBackInvokedCallbackInfo[size];
                }
            };
}
