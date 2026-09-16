extern int local_group_value(void);
int consumer_value(void) { return local_group_value() + 1; }
