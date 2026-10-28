package android.app;
public final class BroadcastOptions {
    private final android.os.Bundle bundle=new android.os.Bundle();
    public static BroadcastOptions makeBasic(){return new BroadcastOptions();}
    public void setTemporaryAppAllowlist(long duration,int type,int reason,String tag){bundle.duration=duration;bundle.type=type;bundle.reason=reason;bundle.tag=tag;}
    public android.os.Bundle toBundle(){return bundle;}
}
