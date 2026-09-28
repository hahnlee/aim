package android.media;

import android.content.Context;
import android.media.session.MediaSessionManager;
import android.view.KeyEvent;

import java.util.Collections;
import java.util.List;
import java.util.concurrent.Executor;

/**
 * Android's media module normally contributes this framework class at boot.
 * The detached runtime currently boots framework.jar without the media apex,
 * so keep MediaSessionManager verifiable and expose its process-local no-op
 * communication surface. Classic MediaSession playback is handled separately
 * through ISessionManager and does not depend on Session2 discovery.
 */
public class MediaCommunicationManager {
    public interface SessionCallback {
        default void onSession2TokenCreated(Session2Token token) {}
        default void onSession2TokenCreated(Session2Token token, int uid) {}
        default void onSession2TokensChanged(List<Session2Token> tokens) {}
    }

    public MediaCommunicationManager(Context context) {}

    public int getVersion() { return 0; }
    public void notifySession2Created(Session2Token token) {}
    public boolean isTrustedForMediaControl(MediaSessionManager.RemoteUserInfo userInfo) {
        return false;
    }
    public List<Session2Token> getSession2Tokens() { return Collections.emptyList(); }
    public void registerSessionCallback(Executor executor, SessionCallback callback) {}
    public void unregisterSessionCallback(SessionCallback callback) {}
    public void dispatchMediaKeyEvent(KeyEvent event, boolean needWakeLock) {}
}
