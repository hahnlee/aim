package com.android.server.media;
import android.content.Context;
import android.media.*;
import android.os.*;
public class SystemMediaRoute2Provider {
 public static int created,refreshed,lastUser;
 public static boolean empty,noSession,fail;
 public static SystemMediaRoute2Provider create(Context c,UserHandle u,Looper l){created++;lastUser=u.id;return new SystemMediaRoute2Provider();}
 public MediaRoute2ProviderInfo getProviderInfo(){return empty?null:new MediaRoute2ProviderInfo();}
 public MediaRoute2Info getDefaultRoute(){return new MediaRoute2Info("default");}
 public RoutingSessionInfo getDefaultSessionInfo(){return new RoutingSessionInfo("default","");}
 public RoutingSessionInfo getSessionForPackage(String p){return noSession?null:new RoutingSessionInfo("system",p);}
 public RoutingSessionInfo generateDeviceRouteSelectedSessionInfo(String p){return noSession?null:new RoutingSessionInfo("device",p);}
 void updateVolume(){if(!Binder.cleared)throw new AssertionError("provider identity");refreshed++;if(fail)throw new IllegalStateException("provider failed");}
}
