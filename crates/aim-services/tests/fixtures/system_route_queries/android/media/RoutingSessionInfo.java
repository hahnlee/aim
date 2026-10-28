package android.media;
public final class RoutingSessionInfo {
 public final String route,client;
 public RoutingSessionInfo(String r,String c){route=r;client=c;}
 public String toString(){return route+":"+client;}
 public static class Builder {
  private String route,client;
  public Builder(RoutingSessionInfo s){route=s.route;client=s.client;}
  public Builder setClientPackageName(String p){client=p;return this;}
  public RoutingSessionInfo build(){return new RoutingSessionInfo(route,client);}
 }
}
