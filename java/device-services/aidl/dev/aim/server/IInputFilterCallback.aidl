package dev.aim.server;
/** InputDispatcher to the retained original input filter (#2529). */
oneway interface IInputFilterCallback {
    void filterInputEvent(in byte[] event, int policyFlags);
}
