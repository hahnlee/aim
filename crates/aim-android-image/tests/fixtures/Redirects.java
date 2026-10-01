package dev.aim.server.test;

import java.text.SimpleDateFormat;
import java.util.Date;
import java.util.Iterator;
import java.util.LinkedList;
import java.util.TreeSet;

/** The targets of the redirect test (tests/system_server.rs). */
public final class Redirects {
    private Redirects() {}

    public static String format(SimpleDateFormat format, Date date) {
        return format.format(date);
    }

    public static Object pollLast(TreeSet<?> set) {
        return set.pollLast();
    }

    public static StringBuilder append(StringBuilder builder, String s) {
        return builder.append(s);
    }

    public static void loadLibrary(String name) {
        System.loadLibrary(name);
    }

    public static void writeAmWtf(int pid, int uid, String process, int flags, String tag,
            String message) {}

    public static boolean add(LinkedList<Object> list, Object o) {
        return list.add(o);
    }

    public static boolean hasNext(Iterator<?> iterator) {
        return iterator.hasNext();
    }

    /** Not static: no target. */
    public boolean instanceHasNext(Iterator<?> iterator) {
        return iterator.hasNext();
    }

    static boolean hiddenHasNext(Iterator<?> iterator) {
        return iterator.hasNext();
    }
}
