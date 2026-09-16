extern int child_value(void);
int parent_value = 37;
int local_group_value(void) { return child_value(); }
