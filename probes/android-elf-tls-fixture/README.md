# Android ELF TLS ART fixture

This API 35 AArch64 DSO self-tests the loader's bounded local-definition
`TLSDESC` model from its exported `JNI_OnLoad`. The loading ART thread mutates
initialized, zero-filled, and 64-byte-aligned TLS. Four Bionic-provider pthreads
must each observe a fresh template, retain independent mutations, and exit
before unload; the loading thread's values must remain unchanged.

The fixture imports only `pthread_create@LIBC` and `pthread_join@LIBC`. It does
not exercise imported/static TLS or thread-local destructors. Those remain
closed capabilities.
