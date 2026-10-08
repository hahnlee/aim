// A stub of the image's class for compiling against (docs/build.md, "Java"):
// the members used, checked against the image by the device-services node.
package android.app;

public abstract class ActivityManagerInternal {
    public abstract void killApplicationSync(String name, int appId, int user, String reason, int exitReason);
    public ActivityManagerInternal() { throw new RuntimeException("stub"); }
    public abstract String getPackageNameByPid(int pid);
    public abstract long getBootTimeTempAllowListDuration();
    public abstract int broadcastIntentWithCallback(android.content.Intent intent,android.content.IIntentReceiver receiver,String[] permissions,int user,int[] appIdAllowList,java.util.function.BiFunction<Integer,android.os.Bundle,android.os.Bundle> filter,android.os.Bundle options);
}
