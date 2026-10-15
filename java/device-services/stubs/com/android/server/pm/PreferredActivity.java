// Compile-only pinned original API.
package com.android.server.pm;
public class PreferredActivity extends WatchedIntentFilter {
    public PreferredActivity(android.content.IntentFilter filter,int match,android.content.ComponentName[] set,android.content.ComponentName activity,boolean always) { throw new RuntimeException("stub"); }
    public PreferredActivity(com.android.modules.utils.TypedXmlPullParser parser) throws java.io.IOException, org.xmlpull.v1.XmlPullParserException {throw new RuntimeException("stub");}
    public void writeToXml(com.android.modules.utils.TypedXmlSerializer writer,boolean full) throws java.io.IOException {throw new RuntimeException("stub");}
    public PreferredActivity snapshot() { throw new RuntimeException("stub"); }
}
