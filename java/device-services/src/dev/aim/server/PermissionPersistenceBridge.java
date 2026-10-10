package dev.aim.server;

import android.util.Xml;
import com.android.server.LocalServices;
import com.android.server.pm.permission.LegacyPermissionSettings;
import com.android.server.pm.permission.PermissionManagerServiceInternal;
import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.util.Objects;

/** Settings.writeLPr/readPermissionStateForUserLPr's actual permission leaves. */
public final class PermissionPersistenceBridge extends IPermissionPersistenceBridge.Stub {
    private PermissionManagerServiceInternal owner() {
        return Objects.requireNonNull(LocalServices.getService(PermissionManagerServiceInternal.class),
                "permission persistence owner unavailable");
    }
    @Override public byte[] captureDefinitions() {
        Bridge.enforceSystemUid();
        var settings = new LegacyPermissionSettings();
        owner().writeLegacyPermissionsTEMP(settings);
        try {
            var bytes = new ByteArrayOutputStream();
            var xml = Xml.resolveSerializer(bytes);
            xml.startDocument(null, true); xml.startTag(null, "packages");
            xml.startTag(null, "permissions"); settings.writePermissions(xml);
            xml.endTag(null, "permissions");
            xml.startTag(null, "permission-trees"); settings.writePermissionTrees(xml);
            xml.endTag(null, "permission-trees");
            xml.endTag(null, "packages"); xml.endDocument();
            return bytes.toByteArray();
        } catch (IOException error) { throw new IllegalStateException("permission definition capture failed", error); }
    }
    @Override public void importNativeMigration() {
        Bridge.enforceSystemUid(); owner().readLegacyPermissionStateTEMP();
    }
}
