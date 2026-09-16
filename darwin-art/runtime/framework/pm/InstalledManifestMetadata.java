package dev.darwinart.runtime.pm;

import android.os.Bundle;
import java.io.ByteArrayOutputStream;
import java.nio.ByteBuffer;
import java.nio.CharBuffer;
import java.nio.charset.CharacterCodingException;
import java.nio.charset.Charset;
import java.nio.charset.CharsetDecoder;
import java.nio.charset.CodingErrorAction;
import java.util.HashSet;

/** Decodes the APK inspector's application meta-data into the framework Bundle. */
public final class InstalledManifestMetadata {
    private static final Charset UTF_8 = Charset.forName("UTF-8");

    private InstalledManifestMetadata() {}

    /** Returns null when the installed record has no application meta-data. */
    public static Bundle fromRecord(InstalledPackageRecord installed) {
        String encoded = installed.legacyManifestHint("application_metadata");
        if (encoded == null || encoded.equals("none")) return null;
        if (encoded.isEmpty()) throw malformed("empty application metadata");

        Bundle result = new Bundle();
        HashSet<String> names = new HashSet<>();
        for (String entry : encoded.split(",", -1)) {
            if (entry.isEmpty()) throw malformed("empty application metadata entry");
            int first = entry.indexOf(':');
            int second = first < 0 ? -1 : entry.indexOf(':', first + 1);
            if (first <= 0 || second != first + 2 || entry.indexOf(':', second + 1) >= 0) {
                throw malformed("invalid application metadata entry");
            }
            String name = decodeUtf8Hex(entry.substring(0, first), "metadata name");
            if (name.isEmpty() || name.indexOf('\0') >= 0 || !names.add(name)) {
                throw malformed("invalid or duplicate application metadata name");
            }
            char kind = entry.charAt(first + 1);
            String data = entry.substring(second + 1);
            switch (kind) {
                case 'r':
                    InstalledResourceValue.put(
                            result, name, installed, parseU32(data, "resource metadata"));
                    break;
                case 'i':
                    result.putInt(name, parseU32(data, "integer metadata"));
                    break;
                case 'b':
                    if (data.equals("1")) {
                        result.putBoolean(name, true);
                    } else if (data.equals("0")) {
                        result.putBoolean(name, false);
                    } else {
                        throw malformed("invalid boolean metadata");
                    }
                    break;
                case 's':
                    result.putString(name, decodeUtf8Hex(data, "string metadata"));
                    break;
                default:
                    throw malformed("unknown application metadata type");
            }
        }
        return result;
    }

    private static int parseU32(String value, String label) {
        if (value.length() != 8) throw malformed("invalid " + label + " width");
        long result = 0;
        for (int index = 0; index < value.length(); index++) {
            int digit = hexDigit(value.charAt(index));
            if (digit < 0) throw malformed("invalid " + label + " hex");
            result = (result << 4) | digit;
        }
        return (int) result;
    }

    private static String decodeUtf8Hex(String value, String label) {
        if ((value.length() & 1) != 0) throw malformed("odd " + label + " hex");
        ByteArrayOutputStream bytes = new ByteArrayOutputStream(value.length() / 2);
        for (int index = 0; index < value.length(); index += 2) {
            int high = hexDigit(value.charAt(index));
            int low = hexDigit(value.charAt(index + 1));
            if (high < 0 || low < 0) throw malformed("invalid " + label + " hex");
            bytes.write((high << 4) | low);
        }
        CharsetDecoder decoder = UTF_8.newDecoder()
                .onMalformedInput(CodingErrorAction.REPORT)
                .onUnmappableCharacter(CodingErrorAction.REPORT);
        try {
            CharBuffer decoded = decoder.decode(ByteBuffer.wrap(bytes.toByteArray()));
            return decoded.toString();
        } catch (CharacterCodingException error) {
            throw new IllegalArgumentException("invalid UTF-8 in " + label, error);
        }
    }

    private static IllegalArgumentException malformed(String message) {
        return new IllegalArgumentException(message);
    }

    private static int hexDigit(char value) {
        if (value >= '0' && value <= '9') return value - '0';
        if (value >= 'a' && value <= 'f') return value - 'a' + 10;
        if (value >= 'A' && value <= 'F') return value - 'A' + 10;
        return -1;
    }
}
