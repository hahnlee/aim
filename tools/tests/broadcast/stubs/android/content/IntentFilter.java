package android.content;

import android.net.Uri;
import java.util.HashSet;
import java.util.Set;

/** Test stub: IntentFilter.match for action and category filters without data. */
public class IntentFilter {
    private final Set<String> actions = new HashSet<>();
    private final Set<String> categories = new HashSet<>();

    public IntentFilter(String action) { actions.add(action); }
    public void addCategory(String category) { categories.add(category); }

    public int match(String action, String type, String scheme, Uri data,
            Set<String> intentCategories, String logTag) {
        if (!actions.contains(action)) return -3; // NO_MATCH_ACTION
        if (intentCategories != null && !categories.containsAll(intentCategories)) return -4;
        return 0x108000; // MATCH_CATEGORY_EMPTY | MATCH_ADJUSTMENT_NORMAL
    }
}
