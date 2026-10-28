package dev.aim.server;
public interface ISystemRouteQueries {
 byte[] getSystemRoutes(int u,int p,String c,boolean proxy);
 byte[] getSystemSessionInfo(int u,int p,String c,String t,boolean selected);
 abstract class Stub implements ISystemRouteQueries {}
}
