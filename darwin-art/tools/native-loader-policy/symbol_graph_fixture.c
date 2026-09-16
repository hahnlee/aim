// Test-only Android ELF graph: root -> left -> deep, root -> right.
#if NODE_KIND == 0
extern int left_anchor(void);
extern int right_anchor(void);
int root_anchor(void) { return left_anchor() + right_anchor(); }
#elif NODE_KIND == 1
extern int deep_anchor(void);
int left_anchor(void) { return deep_anchor(); }
int left_only(void) { return 11; }
#elif NODE_KIND == 2
int right_anchor(void) { return 2; }
int winner(void) { return 22; }
#elif NODE_KIND == 3
int deep_anchor(void) { return 3; }
int deep_only(void) { return 33; }
int winner(void) { return 33; }
#else
#error Missing test node kind
#endif
