package dev.aim.server;
import android.content.Intent;
/** Ordered native consent presentation; null intent cancels the credentials notification. */
oneway interface IAccountConsentNotification {
    void update(in Intent intent, String tag, int uid, String accountName, String packageName);
}
