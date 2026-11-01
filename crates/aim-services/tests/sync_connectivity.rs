//! Execute the pinned retained receiver/read/backoff bodies with boundary doubles.
use sha2::{Digest, Sha256};
use std::{fs, process::Command};
mod common {
    pub mod runtime;
}
#[test]
#[ignore = "requires pinned AOSP source and JDK; run explicitly without a guest boot"]
fn retained_reconnect_rereads_active_network_and_resets_backoffs() {
    let path = aim_paths::aosp()
        .join("frameworks/base/services/core/java/com/android/server/content/SyncManager.java");
    let source = fs::read_to_string(path).unwrap();
    assert_eq!(
        format!("{:x}", Sha256::digest(source.as_bytes())),
        "ac8deedeeca97dadea2d9da8b7e2956305fd63e44d9fdb8d80222ddf76f1ed51"
    );
    let start = source
        .find("    private BroadcastReceiver mConnectivityIntentReceiver =")
        .unwrap();
    let end = source[start..]
        .find("    private String getJobStats()")
        .unwrap()
        + start;
    // Apache-2.0 AOSP android-16.0.0_r1 SyncManager.java. These three original
    // bodies are compiled unchanged; only their surrounding Android owners double.
    let retained = &source[start..end];
    let work = aim_paths::scratch_dir("sync-connectivity-java");
    fs::create_dir_all(&work).unwrap();
    let java = format!(
        r#"
public class Reconnect {{
    static final String TAG = "fixture";
    static class Context {{}}
    static class Intent {{ boolean connected; Intent(boolean c) {{ connected = c; }} }}
    static abstract class BroadcastReceiver {{ abstract void onReceive(Context c, Intent i); }}
    static class Log {{ static final int VERBOSE = 2; static boolean isLoggable(String s, int n) {{ return false; }} }}
    static class Slog {{ static void v(String a, String b) {{}} }}
    static class EndPoint {{ static final Object USER_ALL_PROVIDER_ALL_ACCOUNTS_ALL = new Object(); }}
    static class NetworkInfo {{ boolean isConnected() {{ return true; }} }}
    static class ConnectivityManager {{
        int reads; boolean connected;
        NetworkInfo getActiveNetworkInfo() {{ reads++; return connected ? new NetworkInfo() : null; }}
    }}
    static class Storage {{
        long backoff = 123; int clears;
        void clearAllBackoffsLocked() {{ backoff = -1; clears++; }}
    }}
    boolean mDataConnectionIsConnected;
    Storage mSyncStorageEngine = new Storage();
    ConnectivityManager cm = new ConnectivityManager();
    ConnectivityManager getConnectivityManager() {{ return cm; }}
    int reschedules;
    void rescheduleSyncs(Object endpoint, String why) {{
        if (endpoint != EndPoint.USER_ALL_PROVIDER_ALL_ACCOUNTS_ALL || !"network reconnect".equals(why)) throw new AssertionError();
        reschedules++;
    }}
{retained}
    void event(boolean actual, boolean payload, int clears) {{
        cm.connected = actual;
        int before = cm.reads;
        mConnectivityIntentReceiver.onReceive(new Context(), new Intent(payload));
        if (cm.reads != before + 1 || mDataConnectionIsConnected != actual
            || mSyncStorageEngine.clears != clears || reschedules != clears) throw new AssertionError();
    }}
    public static void main(String[] args) {{
        Reconnect r = new Reconnect();
        r.event(false, true, 0); // No active network despite a connected payload.
        r.event(true, false, 1); // Initial connection clears persisted backoff.
        if (r.mSyncStorageEngine.backoff != -1) throw new AssertionError();
        r.mSyncStorageEngine.backoff = 456;
        r.event(true, false, 1); r.event(true, true, 1); // Replacement pair.
        if (r.mSyncStorageEngine.backoff != 456) throw new AssertionError();
        r.event(false, true, 1); // Genuine loss preserves backoff.
        if (r.mSyncStorageEngine.backoff != 456) throw new AssertionError();
        r.event(true, false, 2); r.event(true, true, 2);
        if (r.mSyncStorageEngine.backoff != -1) throw new AssertionError();
    }}
}}
"#
    );
    let file = work.join("Reconnect.java");
    fs::write(&file, java).unwrap();
    let jdk =
        aim_paths::fetched().join("java/temurin-17.0.20.1+1/jdk-17.0.20.1+1/Contents/Home/bin");
    common::runtime::run(
        Command::new(jdk.join("javac"))
            .arg("-d")
            .arg(&work)
            .arg(file),
    );
    common::runtime::run(
        Command::new(jdk.join("java"))
            .arg("-cp")
            .arg(&work)
            .arg("Reconnect"),
    );
    fs::remove_dir_all(work).unwrap();
}
