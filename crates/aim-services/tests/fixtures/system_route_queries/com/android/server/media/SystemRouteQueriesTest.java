package com.android.server.media;
import android.content.*;
import android.os.*;
import com.android.media.flags.Flags;
import java.nio.charset.StandardCharsets;

public final class SystemRouteQueriesTest {
 static String text(byte[] b){if(Binder.cleared||Parcel.live!=0)throw new AssertionError("cleanup");return new String(b,StandardCharsets.UTF_8);}
 static void equal(Object a,Object b){if(!java.util.Objects.equals(a,b))throw new AssertionError(a+" != "+b);}
 static final String PREFIX="android.permission.";
 public static void main(String[] args){
  Context c=new Context();
  SystemRouteQueries q=new SystemRouteQueries(c);
  equal(text(q.getSystemRoutes(210100,71,"caller",true)),"[default]");
  equal(PermissionChecker.calls,0);
  c.granted.add(PREFIX+"MODIFY_AUDIO_ROUTING");
  equal(text(q.getSystemRoutes(210100,71,"caller",false)),"[device]");
  equal(text(q.getSystemRoutes(210100,71,"caller",true)),"[default]");
  c.granted.clear();c.granted.add(PREFIX+"BLUETOOTH_CONNECT");
  equal(text(q.getSystemRoutes(210100,71,"caller",false)),"[default]");
  if(!c.checks.contains(PREFIX+"BLUETOOTH_SCAN"))throw new AssertionError("both bluetooth checks");
  c.granted.add(PREFIX+"BLUETOOTH_SCAN");
  equal(text(q.getSystemSessionInfo(210100,71,null,null,false)),"system:null");
  equal(text(q.getSystemSessionInfo(210100,71,"caller","target",false)),"default:target");
  c.granted.clear();
  Flags.privileged=true;PermissionChecker.result=-1;
  equal(text(q.getSystemRoutes(210100,71,"caller",true)),"[default]");
  equal(PermissionChecker.calls,1);
  PermissionChecker.result=0;
  equal(text(q.getSystemRoutes(210100,71,"caller",true)),"[device]");
  equal(text(q.getSystemSessionInfo(210100,71,"caller","target",false)),"system:target");
  equal(text(q.getSystemSessionInfo(210100,71,"caller","target",true)),"device:target");
  equal(text(q.getSystemSessionInfo(210100,71,"caller",null,false)),"default:null");
  int calls=PermissionChecker.calls;
  c.granted.add(PREFIX+"MEDIA_CONTENT_CONTROL");
  PermissionChecker.result=-1;
  equal(text(q.getSystemRoutes(210100,71,"caller",true)),"[device]");
  equal(PermissionChecker.calls,calls);
  Flags.privileged=false;
  equal(text(q.getSystemSessionInfo(210100,71,"caller","target",false)),"system:target");
  SystemMediaRoute2Provider.empty=true;
  equal(text(q.getSystemRoutes(210100,71,"caller",true)),"[]");
  SystemMediaRoute2Provider.noSession=true;
  equal(text(q.getSystemSessionInfo(210100,71,"caller","target",false)),"null");
  equal(text(q.getSystemSessionInfo(210100,71,"caller","target",true)),"null");
  SystemMediaRoute2Provider.noSession=false;
  Flags.mirroring=true;
  try{q.getSystemSessionInfo(210100,71,"caller","target",true);throw new AssertionError();}
  catch(UnsupportedOperationException expected){if(!expected.getMessage().contains("#1663"))throw new AssertionError();}
  if(Binder.cleared||Parcel.live!=0)throw new AssertionError("mirroring cleanup");
  Flags.mirroring=false;
  equal(SystemMediaRoute2Provider.created,1);equal(SystemMediaRoute2Provider.lastUser,2);
  SystemMediaRoute2Provider.fail=true;
  try{q.getSystemRoutes(210100,71,"caller",true);throw new AssertionError();}catch(IllegalStateException expected){}
  if(Binder.cleared||Parcel.live!=0)throw new AssertionError("failure cleanup");
  for(int uid:new int[]{0,2000,10100}){
   Binder.uid=uid;
   try{q.getSystemRoutes(210100,71,"caller",true);throw new AssertionError();}catch(SecurityException expected){}
   try{q.getSystemSessionInfo(210100,71,"caller","target",true);throw new AssertionError();}catch(SecurityException expected){}
  }
 }
}
