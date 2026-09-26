package android.app;

/** Test stub. */
public class NotificationChannel {
    private final String id;
    private CharSequence name;
    private String description;
    private String group;
    private int importance;

    public NotificationChannel(String id, CharSequence name, int importance) {
        this.id = id;
        this.name = name;
        this.importance = importance;
    }

    public String getId() { return id; }
    public CharSequence getName() { return name; }
    public void setName(CharSequence value) { name = value; }
    public String getDescription() { return description; }
    public void setDescription(String value) { description = value; }
    public String getGroup() { return group; }
    public void setGroup(String value) { group = value; }
    public int getImportance() { return importance; }
    public void setImportance(int value) { importance = value; }
}
