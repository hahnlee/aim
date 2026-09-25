package android.content;

import android.net.Uri;
import java.util.HashSet;
import java.util.Set;

/** Test stub carrying the Intent fields launch resolution reads. */
public class Intent {
    public static final String ACTION_MAIN = "android.intent.action.MAIN";
    public static final String ACTION_VIEW = "android.intent.action.VIEW";
    public static final String CATEGORY_LAUNCHER = "android.intent.category.LAUNCHER";
    public static final String CATEGORY_INFO = "android.intent.category.INFO";
    private final String action;
    private Set<String> categories;
    private String packageName;

    public Intent(String action) { this.action = action; }
    public Intent addCategory(String category) {
        if (categories == null) categories = new HashSet<>();
        categories.add(category);
        return this;
    }
    public Intent setPackage(String packageName) { this.packageName = packageName; return this; }
    public String getAction() { return action; }
    public Set<String> getCategories() { return categories; }
    public String getPackage() { return packageName; }
    public Uri getData() { return null; }
    public String getType() { return null; }
    public ComponentName getComponent() { return null; }
    public Intent getSelector() { return null; }
}
