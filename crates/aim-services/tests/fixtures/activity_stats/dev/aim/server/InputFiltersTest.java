package dev.aim.server;
import android.view.*;
import java.lang.reflect.Proxy;
public final class InputFiltersTest {
    static IInputFilterCallback callback;
    static int sends, flags;
    static void check(boolean b) { if (!b) throw new AssertionError(); }
    static final class Filter implements IInputFilter {
        IInputFilterHost host; int installs, uninstalls, events;
        public void install(IInputFilterHost h) { host=h; installs++; }
        public void uninstall() { uninstalls++; }
        public void filterInputEvent(InputEvent e,int f) throws android.os.RemoteException {
            events++; host.sendInputEvent(e,f);
        }
    }
    public static void main(String[] args) throws Exception {
        var nativeHost=(IAccessibilityWindowHost)Proxy.newProxyInstance(InputFiltersTest.class.getClassLoader(),
                new Class<?>[]{IAccessibilityWindowHost.class},(p,m,v)-> {
                    if(m.getName().equals("setInputFilter")) callback=(IInputFilterCallback)v[0];
                    else if(m.getName().equals("sendInputEvent")){ sends++; flags=(int)v[1]; }
                    else throw new AssertionError(m); return null;
                });
        var owner=new InputFilters(new android.content.Context(),nativeHost);
        var a=new Filter(); var b=new Filter(); owner.set(a); owner.set(a);
        check(a.installs==1); callback.filterInputEvent(new byte[0],0x42000000);
        check(a.events==1 && sends==1 && flags==0x46000000);
        var stale=callback; owner.set(b); check(a.uninstalls==1 && b.installs==1);
        stale.filterInputEvent(new byte[0],0x42000000);
        a.host.sendInputEvent(new InputEvent(),0x42000000); check(sends==1);
        android.content.Context.allowed=false;
        try { b.host.sendInputEvent(new InputEvent(),0); throw new AssertionError(); }
        catch(SecurityException expected) {} android.content.Context.allowed=true;
        try { b.host.sendInputEvent(null,0); throw new AssertionError(); }
        catch(NullPointerException expected) {}
        owner.set(null); check(callback==null && b.uninstalls==1);
        b.host.sendInputEvent(new InputEvent(),0x42000000); check(sends==1);
    }
}
