package dev.darwinart.runtime.compat;

import android.util.Xml;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.LinkOption;
import java.nio.file.NoSuchFileException;
import java.nio.file.attribute.BasicFileAttributes;
import java.util.ArrayList;
import java.util.Arrays;
import java.util.HashSet;
import java.util.List;
import org.xmlpull.v1.XmlPullParser;

/** Pinned immutable image topology, not a compatibility-policy evaluator. */
final class SystemCompatCatalog {
    private SystemCompatCatalog() {}

    static List<File> files(File system, File apex) throws Exception {
        ArrayList<File> files = new ArrayList<>();
        append(files, new File(system, "etc/compatconfig"), false);
        append(files, new File(system, "system_ext/etc/compatconfig"), false);
        directory(apex);
        File inventory = new File(apex, "apex-info-list.xml");
        regular(inventory);
        HashSet<String> active = new HashSet<>();
        try (FileInputStream input = new FileInputStream(inventory)) {
            XmlPullParser parser = Xml.newPullParser();
            parser.setInput(input, null);
            if (parser.nextTag() != XmlPullParser.START_TAG
                    || !"apex-info-list".equals(parser.getName())) {
                throw new IOException("Invalid active APEX inventory root");
            }
            while (parser.nextTag() == XmlPullParser.START_TAG) {
                if (parser.getDepth() != 2 || !"apex-info".equals(parser.getName())) {
                    throw new IOException("Unexpected active APEX inventory entry");
                }
                String enabled = parser.getAttributeValue(null, "isActive");
                String name = parser.getAttributeValue(null, "moduleName");
                if (!("true".equals(enabled) || "false".equals(enabled))
                        || !moduleName(name)) {
                    throw new IOException("Invalid active APEX inventory attributes");
                }
                if ("true".equals(enabled)) {
                    if (!active.add(name)) throw new IOException("Duplicate active APEX module");
                    directory(new File(apex, name));
                }
                if (parser.nextTag() != XmlPullParser.END_TAG || parser.getDepth() != 2) {
                    throw new IOException("Nested active APEX inventory entry");
                }
            }
            if (parser.getEventType() != XmlPullParser.END_TAG || parser.getDepth() != 1
                    || !"apex-info-list".equals(parser.getName())) {
                throw new IOException("Invalid active APEX inventory termination");
            }
            int trailing;
            do {
                trailing = parser.next();
            } while (trailing == XmlPullParser.TEXT && parser.isWhitespace());
            if (trailing != XmlPullParser.END_DOCUMENT) {
                throw new IOException("Trailing active APEX inventory content");
            }
        }
        String[] modules = active.toArray(new String[0]);
        Arrays.sort(modules);
        for (String name : modules) append(files, new File(apex, name + "/etc/compatconfig"), true);
        return files;
    }

    private static boolean moduleName(String value) {
        return value != null && !value.isEmpty() && !".".equals(value) && !"..".equals(value)
                && value.matches("[A-Za-z0-9_.-]+");
    }

    private static BasicFileAttributes attributes(File file) throws IOException {
        return Files.readAttributes(file.toPath(), BasicFileAttributes.class,
                LinkOption.NOFOLLOW_LINKS);
    }

    private static void directory(File file) throws IOException {
        if (!attributes(file).isDirectory()) throw new IOException("Not a catalog directory: " + file);
    }

    private static void regular(File file) throws IOException {
        if (!attributes(file).isRegularFile()) throw new IOException("Not a catalog file: " + file);
    }

    private static void append(List<File> files, File directory, boolean optional) throws IOException {
        try {
            directory(directory);
        } catch (NoSuchFileException missing) {
            if (optional) return;
            throw missing;
        }
        File[] entries = directory.listFiles();
        if (entries == null) throw new IOException("Cannot enumerate compatibility catalog: " + directory);
        Arrays.sort(entries, (a, b) -> a.getName().compareTo(b.getName()));
        for (File entry : entries) {
            if (!entry.getName().endsWith(".xml")) throw new IOException("Unexpected catalog entry: " + entry);
            regular(entry);
            files.add(entry);
        }
    }
}
