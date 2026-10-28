import android.content.Context;
import android.content.UriRelativeFilterGroup;
import android.content.UriRelativeFilterGroupParcel;
import android.content.pm.PackageManager.NameNotFoundException;
import android.os.Bundle;
import com.android.server.pm.verify.domain.AppHostUriRelativeFilterGroups;
import dev.aim.server.IAppHostDomainVerification;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.List;

public class UriGroupsTest {
    static final class Backend implements IAppHostDomainVerification {
        final Bundle state = new Bundle();
        int writes;
        public String getDomainVerificationPackageId(String name) { return "p".equals(name) ? "id" : null; }
        public void setUriRelativeFilterGroup(String name, String id, String domain, List<UriRelativeFilterGroupParcel> groups) {
            if (!name.equals("p") || !id.equals("id")) throw new AssertionError(name);
            writes++;
            if (groups.isEmpty()) state.values.remove(domain);
            else state.values.put(domain, new ArrayList<>(groups));
        }
        public Bundle getUriRelativeFilterGroups(int uid, String name, List<String> domains) {
            if (uid != 10100 || !name.equals("p")) throw new AssertionError("wrong query identity");
            Bundle result = new Bundle();
            for (String domain : domains) if (state.values.containsKey(domain))
                result.values.put(domain, state.values.get(domain));
            return result;
        }
    }
    interface Action { void run() throws Exception; }
    static void expect(Class<? extends Exception> type, Action action) throws Exception {
        try { action.run(); } catch (Exception error) {
            if (type.isInstance(error)) return;
            throw error;
        }
        throw new AssertionError("expected " + type);
    }
    static Bundle bundle(String key, Object value) {
        Bundle bundle = new Bundle(); bundle.values.put(key, value); return bundle;
    }
    static void check(boolean value) { if (!value) throw new AssertionError(); }
    public static void main(String[] args) throws Exception {
        Context context = new Context();
        Backend backend = new Backend();
        var parcel = new UriRelativeFilterGroupParcel(); parcel.action = 1;
        var groups = new ArrayList<>(List.of(parcel));
        context.granted = false;
        expect(SecurityException.class, () -> AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", null));
        check(backend.writes == 0);
        context.granted = true;
        expect(NullPointerException.class, () -> AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", null));
        AppHostUriRelativeFilterGroups.set(context, 10100, backend, "missing", new Bundle());
        expect(NameNotFoundException.class, () -> AppHostUriRelativeFilterGroups.set(context, 10100, backend, "missing", bundle("a.org", groups)));
        Bundle set = bundle("a.org", groups); set.values.put("invalid", groups);
        AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", set);
        check(set.getters == 1 && UriRelativeFilterGroup.conversions == 1);
        check(AppHostUriRelativeFilterGroups.get(backend, 10100, "p", List.of("a.org", "missing.org")).values.size() == 1);
        // Typed-getter mismatch and empty lists both remove the key. The
        // adapter must leave Serializable/class handling to original Bundle.
        AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", bundle("a.org", "mismatch"));
        check(backend.state.isEmpty());
        AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", bundle("a.org", groups));
        AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", bundle("a.org", new ArrayList<>()));
        check(backend.state.isEmpty());
        Bundle partial = bundle("a.org", groups);
        partial.values.put("b.org", new ArrayList<>(Arrays.asList((UriRelativeFilterGroupParcel)null)));
        partial.values.put("c.org", groups);
        expect(NullPointerException.class, () -> AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", partial));
        check(backend.state.values.keySet().equals(java.util.Set.of("a.org")));
        expect(StringIndexOutOfBoundsException.class, () -> AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", bundle("", groups)));
        expect(NullPointerException.class, () -> AppHostUriRelativeFilterGroups.set(context, 10100, backend, "p", bundle(null, groups)));
        check(AppHostUriRelativeFilterGroups.get(backend, 10100, "missing", null).isEmpty());
        expect(NullPointerException.class, () -> AppHostUriRelativeFilterGroups.get(backend, 10100, "p", null));
        // Reads have no agent permission check in original DVS.
        int checks = context.checks; context.granted = false;
        AppHostUriRelativeFilterGroups.get(backend, 10100, "p", List.of("a.org"));
        check(context.checks == checks);
    }
}
