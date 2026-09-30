package dev.aim.notificationpermission;

import android.app.Activity;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.os.Bundle;
import android.os.IBinder;
import android.os.RemoteException;

import java.util.Arrays;

import dev.aim.server.INotificationPermissionCallback;
import dev.aim.server.INotificationPermissionRequest;

/**
 * Started by system_server in place of PermissionController's dialog for
 * a request of POST_NOTIFICATIONS alone (#470; NotificationPermissionInterceptor
 * of java/device-services). It shows nothing: it asks the Mac through the
 * request it was handed and returns PermissionController's result.
 */
public final class RequestActivity extends Activity {
    private static final String EXTRA_REQUEST = "dev.aim.extra.NOTIFICATION_PERMISSION_REQUEST";

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        String[] names = getIntent().getStringArrayExtra(
                PackageManager.EXTRA_REQUEST_PERMISSIONS_NAMES);
        IBinder request = getIntent().getIBinderExtra(EXTRA_REQUEST);
        if (names == null || request == null) {
            finish();
            return;
        }
        try {
            INotificationPermissionRequest.Stub.asInterface(request).ask(
                    new INotificationPermissionCallback.Stub() {
                        @Override
                        public void onResult(boolean granted) {
                            int[] results = new int[names.length];
                            Arrays.fill(results, granted
                                    ? PackageManager.PERMISSION_GRANTED
                                    : PackageManager.PERMISSION_DENIED);
                            runOnUiThread(() -> answer(names, results));
                        }

                        @Override
                        public void onIgnored() {
                            runOnUiThread(() -> answer(new String[0], new int[0]));
                        }
                    });
        } catch (RemoteException e) {
            finish();
        }
    }

    private void answer(String[] names, int[] results) {
        Intent result = new Intent();
        result.putExtra(PackageManager.EXTRA_REQUEST_PERMISSIONS_NAMES, names);
        result.putExtra(PackageManager.EXTRA_REQUEST_PERMISSIONS_RESULTS, results);
        setResult(RESULT_OK, result);
        finish();
    }
}
