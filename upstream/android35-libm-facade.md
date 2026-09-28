# NDK r28c API 35 libm dependency and first Darwin facade

The pinned ARM64 `libc++_shared.so` retains `libm.so` in `DT_NEEDED`, but it has
no undefined dynamic symbol supplied by the API 35 libm stub and no `libm.so`
entry in `DT_VERNEED`. Its exact libm used subset is therefore empty. The gate
locks this result rather than treating `DT_NEEDED` alone as evidence that a
math symbol is required.

The first executable facade is driven by a separate NDK API 35 fixture. Its
complete libm used subset is exactly:

```text
copysign@LIBC
copysignf@LIBC
fabs@LIBC
fabsf@LIBC
```

These four fixed float/double signatures are implemented as prefixed C ABI
functions using explicit IEEE-754 bit operations. They do not call or interpose
Darwin libm. The explicit resolver accepts only these names and Android GNU
version `LIBC` (or an unversioned default lookup). Unknown names and versions
return zero; `dlsym` and the process-global Mach-O namespace are never used.

The differential smoke covers positive/negative quiet and signaling NaN payloads,
infinities, signed zero, and subnormals against Darwin's fixed-signature APIs.
It also proves all four rounding modes produce identical bits and that existing
`errno` and floating-point exception flags are unchanged.

Capability categories intentionally remain closed:

- `BIT_EXACT`: only the four resolver-visible functions above.
- `RESULT_ONLY_FENV_UNPROVEN`: floor/ceil/trunc/round float and double families.
- `ERRNO_OR_FENV_SENSITIVE`: roots, powers, logs, exponentials, remainders, and
  trigonometric functions. Matching ordinary results is not sufficient.
- `UNSUPPORTED_ABI`: long-double arguments/results and C complex functions,
  including names such as `nexttoward` whose long-double argument is not shown
  by an `l` suffix.

Run:

```sh
tools/build-android35-libm-facade.sh
```

Expanding the resolver requires an Android-vs-Darwin oracle for result bits,
domain/range `errno`, raised exceptions, rounding modes, and ABI—not merely a
matching function name.

