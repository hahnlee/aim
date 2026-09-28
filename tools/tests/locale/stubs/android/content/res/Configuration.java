package android.content.res;

import android.os.LocaleList;

/** Test-only Configuration exposing the Android-owned system locales. */
public final class Configuration {
    public LocaleList getLocales() {
        return LocaleList.getEmptyLocaleList();
    }
}
