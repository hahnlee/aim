// Compile-only pinned image API; never packaged.
package com.android.server.media;
import android.content.Context;
import android.media.MediaRoute2Info;
import android.media.RoutingSessionInfo;
import android.os.Looper;
import android.os.UserHandle;
public class SystemMediaRoute2Provider extends MediaRoute2Provider {
 protected SystemMediaRoute2Provider(Context c,android.content.ComponentName n,UserHandle u,Looper l){super(n,true);throw new RuntimeException("stub");}
 public static SystemMediaRoute2Provider create(Context c,UserHandle u,Looper l){throw new RuntimeException("stub");}
 public MediaRoute2Info getDefaultRoute(){throw new RuntimeException("stub");}
 public RoutingSessionInfo getDefaultSessionInfo(){throw new RuntimeException("stub");}
 public RoutingSessionInfo getSessionForPackage(String p){throw new RuntimeException("stub");}
 public RoutingSessionInfo generateDeviceRouteSelectedSessionInfo(String p){throw new RuntimeException("stub");}
 void updateVolume(){throw new RuntimeException("stub");}
}
