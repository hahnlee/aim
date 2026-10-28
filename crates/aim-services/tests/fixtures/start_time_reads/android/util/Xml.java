package android.util;

/** StAX boundary for the production reader; original policy helpers are observed separately. */
public class Xml {
    public static com.android.modules.utils.TypedXmlPullParser resolvePullParser(java.io.InputStream input)
            throws java.io.IOException {
        try {
            var stream = javax.xml.stream.XMLInputFactory.newFactory().createXMLStreamReader(input);
            int[] depth = {0, 0};
            return (com.android.modules.utils.TypedXmlPullParser) java.lang.reflect.Proxy.newProxyInstance(
                    Xml.class.getClassLoader(), new Class<?>[]{com.android.modules.utils.TypedXmlPullParser.class},
                    (proxy, method, args) -> {
                        switch (method.getName()) {
                            case "next":
                                if (depth[1] == 3) depth[0]--;
                                while (stream.hasNext()) {
                                    int event = stream.next();
                                    if (event == javax.xml.stream.XMLStreamConstants.START_ELEMENT) {
                                        depth[0]++; depth[1] = 2; return 2;
                                    }
                                    if (event == javax.xml.stream.XMLStreamConstants.END_ELEMENT) {
                                        depth[1] = 3; return 3;
                                    }
                                }
                                depth[1] = 1; return 1;
                            case "getDepth": return depth[0];
                            case "getName": return stream.getLocalName();
                            case "getAttributeValue": return stream.getAttributeValue((String) args[0], (String) args[1]);
                            default: throw new AssertionError(method);
                        }
                    });
        } catch (javax.xml.stream.XMLStreamException error) { throw new java.io.IOException(error); }
    }
}
