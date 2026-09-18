package dev.darwinart.runtime.compat;

import android.util.Xml;
import com.android.server.compat.config.Config;
import com.android.server.compat.config.XmlParser;
import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import org.xmlpull.v1.XmlPullParser;

/** Bounded document framing; schema and policy remain in the original parser. */
final class SystemCompatConfigReader {
    private static final int MAX_BYTES = 2 * 1024 * 1024;
    private SystemCompatConfigReader() {}

    static Config read(File file) throws Exception {
        ByteArrayOutputStream bytes = new ByteArrayOutputStream();
        try (FileInputStream input = new FileInputStream(file)) {
            byte[] buffer = new byte[8192];
            int count;
            while ((count = input.read(buffer)) != -1) {
                if (count > MAX_BYTES - bytes.size()) throw new IOException("Oversized compat XML: " + file);
                bytes.write(buffer, 0, count);
            }
        }
        byte[] payload = bytes.toByteArray();
        validate(payload);
        Config config = XmlParser.read(new ByteArrayInputStream(payload));
        if (config == null) throw new IOException("Invalid compatibility config: " + file);
        return config;
    }

    static void validate(byte[] payload) throws Exception {
        if (payload.length > MAX_BYTES) throw new IOException("Oversized compat XML");
        XmlPullParser parser = Xml.newPullParser();
        parser.setInput(new ByteArrayInputStream(payload), null);
        boolean root = false;
        boolean closed = false;
        int event;
        while ((event = parser.nextToken()) != XmlPullParser.END_DOCUMENT) {
            if (event == XmlPullParser.DOCDECL) throw new IOException("Compat XML document type is unsupported");
            if (event == XmlPullParser.START_TAG && parser.getDepth() == 1) {
                if (root || !"config".equals(parser.getName())) throw new IOException("Invalid compat XML root");
                root = true;
            }
            if (event == XmlPullParser.END_TAG && parser.getDepth() == 1) closed = true;
            if (event == XmlPullParser.TEXT && parser.getDepth() == 0 && !parser.isWhitespace()) {
                throw new IOException("Trailing compat XML content");
            }
        }
        if (!root || !closed) throw new IOException("Incomplete compat XML document");
    }
}
