//! Original Android 16 UiAutomationManager, with its Android boundaries doubled on a JVM.
use std::{fs, process::Command};
mod common {
    pub mod runtime;
}

#[test]
#[ignore = "requires pinned JDK and aidl-gen sources; run explicitly"]
fn original_automation_registration_death_and_queued_disconnect() {
    let data = common::runtime::Data(
        aim_paths::out().join(format!("automation-test-{}", std::process::id())),
    );
    fs::create_dir(&data.0).unwrap();
    let mut sources = Vec::new();
    let mut write = |package: &str, name: &str, body: &str| {
        let directory = data.0.join(package.replace('.', "/"));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join(format!("{name}.java"));
        fs::write(&path, format!("package {package};\n{body}\n")).unwrap();
        sources.push(path);
    };
    for name in [
        "Nullable",
        "PermissionManuallyEnforced",
        "RequiresNoPermission",
    ] {
        write(
            "android.annotation",
            name,
            &format!("public @interface {name} {{}}"),
        );
    }
    write(
        "android.os",
        "RemoteException",
        "public class RemoteException extends Exception {}",
    );
    write(
        "android.os",
        "IBinder",
        "public interface IBinder { interface DeathRecipient { void binderDied(); } void linkToDeath(DeathRecipient r,int flags) throws RemoteException; boolean unlinkToDeath(DeathRecipient r,int flags); }",
    );
    write(
        "android.os",
        "Binder",
        r#"public class Binder implements IBinder {
        private final java.util.Set<DeathRecipient> deaths = new java.util.HashSet<>();
        public void linkToDeath(DeathRecipient r,int f) { deaths.add(r); }
        public boolean unlinkToDeath(DeathRecipient r,int f) { return deaths.remove(r); }
        public void die() { for (var r : java.util.List.copyOf(deaths)) r.binderDied(); deaths.clear(); }
        public static UserHandle getCallingUserHandle() { return new UserHandle(); }
    }"#,
    );
    write(
        "android.os",
        "UserHandle",
        "public class UserHandle { public int getIdentifier(){return 0;} }",
    );
    write(
        "android.os",
        "Build",
        "public class Build { public static final boolean IS_USERDEBUG=true, IS_ENG=false; }",
    );
    write(
        "android.os",
        "Looper",
        "public class Looper { static final Looper MAIN=new Looper(); public static Looper getMainLooper(){return MAIN;} }",
    );
    write(
        "android.os",
        "Handler",
        "public class Handler { public final java.util.ArrayDeque<Runnable> queue=new java.util.ArrayDeque<>(); public Looper getLooper(){return Looper.getMainLooper();} public boolean post(Runnable r){queue.add(r);return true;} public void drain(){while(!queue.isEmpty())queue.remove().run();} }",
    );
    write(
        "android.os",
        "RemoteCallback",
        "public class RemoteCallback {}",
    );
    write("android.content", "Context", "public class Context {}");
    write(
        "android.content",
        "ComponentName",
        "public class ComponentName { public ComponentName(String p,String c){} }",
    );
    write(
        "android.app",
        "UiAutomation",
        "public class UiAutomation { public static final int FLAG_DONT_USE_ACCESSIBILITY=2, FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES=1; }",
    );
    write(
        "android.accessibilityservice",
        "AccessibilityService",
        "public class AccessibilityService { public static class SoftKeyboardController { public static final int ENABLE_IME_FAIL_UNKNOWN=2; } }",
    );
    write(
        "android.accessibilityservice",
        "AccessibilityServiceInfo",
        "public class AccessibilityServiceInfo { public void setComponentName(android.content.ComponentName n){} public String getId(){return \"test\";} }",
    );
    write(
        "android.accessibilityservice",
        "AccessibilityTrace",
        "public class AccessibilityTrace { public static final long FLAGS_ACCESSIBILITY_SERVICE_CLIENT=1; public boolean isA11yTracingEnabledForTypes(long f){return false;} public void logTrace(String a,long f,String b){throw new AssertionError();} }",
    );
    write(
        "android.accessibilityservice",
        "IAccessibilityServiceClient",
        "public interface IAccessibilityServiceClient { android.os.IBinder asBinder(); void init(Object connection,int id,android.os.IBinder token) throws android.os.RemoteException; }",
    );
    write(
        "android.view",
        "Display",
        "public class Display { public static final int DEFAULT_DISPLAY=0; }",
    );
    write(
        "android.view.accessibility",
        "AccessibilityEvent",
        "public class AccessibilityEvent { public static String eventTypeToString(int type){return \"events\";} }",
    );
    write(
        "android.util",
        "Slog",
        "public class Slog { public static void v(String t,String m){} public static void e(String t,String m){} public static void e(String t,String m,Throwable e){} public static void w(String t,String m,Throwable e){} }",
    );
    write(
        "com.android.server.utils",
        "Slogf",
        "public class Slogf { public static void i(String t,String f,Object... args){} }",
    );
    write(
        "com.android.internal.util",
        "DumpUtils",
        "public class DumpUtils { public static boolean checkDumpPermission(android.content.Context c,String t,java.io.PrintWriter p){return true;} }",
    );
    write(
        "com.android.internal.util",
        "Preconditions",
        "public class Preconditions { public static void checkArgument(boolean ok,String error){if(!ok)throw new IllegalArgumentException(error);} }",
    );
    write(
        "com.android.server.wm",
        "WindowManagerInternal",
        "public class WindowManagerInternal {}",
    );
    for name in [
        "AccessibilitySecurityPolicy",
        "SystemActionPerformer",
        "AccessibilityWindowManager",
    ] {
        write(
            "com.android.server.accessibility",
            name,
            &format!("public class {name} {{}}"),
        );
    }
    write(
        "com.android.server.accessibility",
        "AbstractAccessibilityServiceConnection",
        r#"
        public abstract class AbstractAccessibilityServiceConnection implements android.os.IBinder.DeathRecipient {
            interface SystemSupport { void onClientChangeLocked(boolean services); }
            static final int DISPLAY_TYPE_DEFAULT=1, DISPLAY_TYPE_PROXY=2;
            android.accessibilityservice.IAccessibilityServiceClient mClient;
            android.os.IBinder mClientBinder;
            final android.content.Context mContext; final Object mLock; final int mId;
            final android.accessibilityservice.AccessibilityTrace mTrace;
            boolean mRequestTouchExplorationMode, mRetrieveInteractiveWindows;
            int mEventTypes=-1; long mNotificationTimeout;
            final java.util.Map<Integer,android.os.IBinder> mOverlayWindowTokens=new java.util.HashMap<>();
            AbstractAccessibilityServiceConnection(android.content.Context c, android.content.ComponentName n,
                android.accessibilityservice.AccessibilityServiceInfo i,int id,android.os.Handler h,Object lock,
                AccessibilitySecurityPolicy p,SystemSupport s,android.accessibilityservice.AccessibilityTrace t,
                com.android.server.wm.WindowManagerInternal w,SystemActionPerformer a,AccessibilityWindowManager awm) {mContext=c;mLock=lock;mId=id;mTrace=t;}
            void setDisplayTypes(int types){} void notifyAccessibilityEvent(android.view.accessibility.AccessibilityEvent e){}
            int getRelevantEventTypes(){return mEventTypes;} android.accessibilityservice.AccessibilityServiceInfo getServiceInfo(){return new android.accessibilityservice.AccessibilityServiceInfo();}
            void addWindowTokensForAllDisplays(){mOverlayWindowTokens.put(0,new android.os.Binder());}
            void onRemoved(){mOverlayWindowTokens.clear();}
            void resetLocked(){try{mClient.init(null,mId,null);}catch(android.os.RemoteException e){throw new AssertionError(e);}mClient=null;}
            protected abstract boolean hasRightsToCurrentUserLocked();
            protected abstract boolean supportsFlagForNotImportantViews(android.accessibilityservice.AccessibilityServiceInfo info);
            public abstract void dump(java.io.FileDescriptor fd,java.io.PrintWriter pw,String[] args);
            public abstract boolean setSoftKeyboardShowMode(int mode); public abstract int getSoftKeyboardShowMode();
            public abstract boolean switchToInputMethod(String id); public abstract int setInputMethodEnabled(String id,boolean enabled);
            public abstract boolean isAccessibilityButtonAvailable(); public abstract void disableSelf();
            public abstract void onServiceConnected(android.content.ComponentName name,android.os.IBinder service);
            public abstract void onServiceDisconnected(android.content.ComponentName name);
            public abstract boolean isCapturingFingerprintGestures(); public abstract void onFingerprintGestureDetectionActiveChanged(boolean active);
            public abstract void onFingerprintGesture(int gesture); public abstract void takeScreenshot(int display,android.os.RemoteCallback callback);
        }"#,
    );
    write(
        "com.android.server.accessibility",
        "AutomationTest",
        r#"
        import android.os.*; import android.accessibilityservice.*;
        public class AutomationTest {
            static class Client extends Binder implements IAccessibilityServiceClient {
                int id=-1, connects, disconnects; Object connection; IBinder token;
                public IBinder asBinder(){return this;}
                public void init(Object c,int i,IBinder t){connection=c;id=i;token=t;if(c==null)disconnects++;else connects++;}
            }
            static final Handler handler=new Handler(); static final Object lock=new Object();
            static int changes;
            static UiAutomationManager manager=new UiAutomationManager(lock);
            static void register(Binder owner,Client client,int id,int flags){
                manager.registerUiTestAutomationServiceLocked(owner,client,new android.content.Context(),
                    new AccessibilityServiceInfo(),id,handler,new AccessibilitySecurityPolicy(),
                    changed -> changes++,new AccessibilityTrace(),new com.android.server.wm.WindowManagerInternal(),
                    new SystemActionPerformer(),new AccessibilityWindowManager(),flags);
            }
            static void check(boolean ok){if(!ok)throw new AssertionError();}
            static void rejects(Runnable action){try{action.run();throw new AssertionError();}catch(IllegalStateException expected){}}
            public static void main(String[] args){
                Binder owner=new Binder(); Client client=new Client();
                register(owner,client,41,0); check(manager.isUiAutomationRunningLocked());
                check(manager.suppressingAccessibilityServicesLocked()); check(client.connects==0);
                rejects(() -> register(new Binder(),new Client(),42,0));
                rejects(() -> manager.unregisterUiTestAutomationServiceLocked(new Client()));
                handler.drain(); check(client.id==41 && client.connection!=null && client.token!=null);
                manager.unregisterUiTestAutomationServiceLocked(client);
                check(client.disconnects==1 && client.connection==null && client.token==null);
                rejects(() -> manager.unregisterUiTestAutomationServiceLocked(client));
                register(new Binder(),client,43,0); handler.drain(); check(client.id==43);
                client.die(); check(!manager.isUiAutomationRunningLocked()); check(client.disconnects==2);
                owner=new Binder(); register(owner,client,44,0); handler.drain();
                owner.die(); check(!manager.isUiAutomationRunningLocked()); check(client.disconnects==3);
                register(new Binder(),client,45,0); manager.unregisterUiTestAutomationServiceLocked(client);
                handler.drain(); check(client.connects==3 && client.disconnects==4);
                register(new Binder(),client,46,2); handler.drain();
                check(manager.isUiAutomationRunningLocked() && !manager.canIntrospect());
                check(client.connects==3); manager.unregisterUiTestAutomationServiceLocked(client);
                check(!manager.isUiAutomationRunningLocked()); check(changes==5);
                System.out.println("original UiAutomationManager lifecycle: PASS");
            }
        }"#,
    );
    let original = aim_paths::aosp().join("frameworks/base/services/accessibility/java/com/android/server/accessibility/UiAutomationManager.java");
    assert!(original.exists(), "run cargo aim build aidl-gen");
    sources.push(original);
    let jdk =
        aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin");
    common::runtime::run(
        Command::new(jdk.join("javac"))
            .args(["--release", "17", "-d"])
            .arg(&data.0)
            .args(&sources),
    );
    common::runtime::run(
        Command::new(jdk.join("java"))
            .arg("-cp")
            .arg(&data.0)
            .arg("com.android.server.accessibility.AutomationTest"),
    );
}
