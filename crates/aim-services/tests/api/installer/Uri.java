// A stub of the image's class for compiling against (docs/build.md, "Java"):
// Compile only; installer_parcel verifies linkage against the original boot classpath.
package android.net;

public abstract class Uri {
    public static Uri parse(String uriString) { throw new RuntimeException("stub"); }
    public abstract String getSchemeSpecificPart();
public static Uri fromParts(String scheme,String ssp,String fragment){throw new RuntimeException("stub");}
public static final class Builder {
public Builder(){throw new RuntimeException("stub");}
public Builder scheme(String value){throw new RuntimeException("stub");}
public Builder authority(String value){throw new RuntimeException("stub");}
public Builder path(String value){throw new RuntimeException("stub");}
public Builder query(String value){throw new RuntimeException("stub");}
public Builder fragment(String value){throw new RuntimeException("stub");}
public Uri build(){throw new RuntimeException("stub");}
}
}
