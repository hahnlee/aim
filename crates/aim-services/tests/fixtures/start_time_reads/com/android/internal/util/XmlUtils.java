package com.android.internal.util;
public class XmlUtils {
    public static boolean nextElementWithin(org.xmlpull.v1.XmlPullParser parser, int depth)
            throws java.io.IOException, org.xmlpull.v1.XmlPullParserException {
        int type;
        while ((type = parser.next()) != 1 && (type != 3 || parser.getDepth() > depth))
            if (type == 2 && parser.getDepth() == depth + 1) return true;
        return false;
    }
    public static void skipCurrentTag(org.xmlpull.v1.XmlPullParser parser)
            throws java.io.IOException, org.xmlpull.v1.XmlPullParserException {
        int depth = parser.getDepth(), type;
        while ((type = parser.next()) != 1 && (type != 3 || parser.getDepth() > depth)) {}
    }
}
