package dev.aim.server;
import android.app.ActivityManagerInternal;
import android.content.Intent;
import android.net.Uri;
import android.os.Binder;
import android.os.Bundle;
import android.os.Parcel;
import android.os.Process;
import android.os.SystemClock;
import android.os.UserHandle;
import com.android.server.LocalServices;
/** Original wire/event and AM owners; package state/visibility belongs to native PMS. */
public final class PackageInternalEventsBridge extends IPackageInternalEventsBridge.Stub {
    private static void enforceOwner(){if(Binder.getCallingUid()!=Process.SYSTEM_UID)throw new SecurityException("native internal package event owner required");}
    private static byte[] parcel(android.os.Parcelable value){Parcel p=Parcel.obtain();try{value.writeToParcel(p,0);return p.marshall();}finally{p.recycle();}}
    private static byte[] record(Intent intent,boolean async){
        Bundle callback=new Bundle();callback.putParcelable(android.content.pm.PackageManager.EXTRA_PACKAGE_MONITOR_CALLBACK_RESULT,intent);
        Parcel p=Parcel.obtain();try{p.writeInt(async?1:0);p.writeByteArray(parcel(intent));p.writeByteArray(parcel(callback));return p.marshall();}finally{p.recycle();}}
    @Override public byte[] prepareRestarted(String name,int uid,int flags){enforceOwner();
        Intent intent=new Intent("android.intent.action.PACKAGE_RESTARTED",Uri.fromParts("package",name,null));
        intent.setFlags(flags|0x04000000);intent.putExtra(Intent.EXTRA_UID,uid);intent.putExtra(Intent.EXTRA_USER_HANDLE,UserHandle.getUserId(uid));
        boolean async=com.android.internal.hidden_from_bootclasspath.android.content.pm.Flags.stayStopped();if(async)intent.putExtra("android.intent.extra.TIME",SystemClock.elapsedRealtime());
        return record(intent,async);
    }
    @Override public byte[] prepareDataCleared(String name,int uid,int user,boolean restore,boolean instant){enforceOwner();
        Intent intent=new Intent("android.intent.action.PACKAGE_DATA_CLEARED",Uri.fromParts("package",name,null));
        intent.addFlags(0x01000000|0x04000000);intent.putExtra(Intent.EXTRA_UID,uid);intent.putExtra(Intent.EXTRA_USER_HANDLE,user);
        if(restore)intent.putExtra("android.intent.extra.IS_RESTORE",true);if(instant)intent.putExtra("android.intent.extra.PACKAGE_NAME",name);
        return record(intent,false);
    }
    @Override public void broadcast(byte[] bytes,int user,boolean instant,int[] allow){enforceOwner();
        Parcel p=Parcel.obtain();Intent intent;try{p.unmarshall(bytes,0,bytes.length);p.setDataPosition(0);intent=Intent.CREATOR.createFromParcel(p);}finally{p.recycle();}
        ActivityManagerInternal am=LocalServices.getService(ActivityManagerInternal.class);if(am==null)throw new IllegalStateException("actual ActivityManagerInternal owner unavailable");
        am.broadcastIntentWithCallback(intent,null,instant?new String[]{"android.permission.ACCESS_INSTANT_APPS"}:null,user,allow,null,null);
    }
}
