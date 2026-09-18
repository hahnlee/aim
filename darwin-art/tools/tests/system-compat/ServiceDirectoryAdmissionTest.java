package dev.darwinart.runtime.system;
import android.os.Binder;
public final class ServiceDirectoryAdmissionTest {
    public static void main(String[] args) {
        ServiceDirectoryAdmission gate = new ServiceDirectoryAdmission();
        gate.enforceLookup();
        Binder.caller = 200;
        try { gate.enforceLookup(); throw new AssertionError("external bootstrap lookup accepted"); }
        catch (SecurityException expected) {}
        try { gate.publish(); throw new AssertionError("external publication accepted"); }
        catch (SecurityException expected) {}
        Binder.caller = 100;
        gate.publish();
        Binder.caller = 200;
        gate.enforceLookup();
        Binder.caller = 100;
        try { gate.publish(); throw new AssertionError("duplicate publication accepted"); }
        catch (IllegalStateException expected) {}
        System.out.println("service directory admission: self bootstrap/external denial/one publication PASS (isolated caller boundary)");
    }
}
