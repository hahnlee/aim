#include <errno.h>
#include <inttypes.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>

typedef union { long double value; uint64_t word[2]; } Binary128;
_Static_assert(sizeof(long double) == 16, "Android binary128 required");

__attribute__((noinline)) static int mixed(const char* input, const char* format, ...) {
  va_list ap;
  va_start(ap, format);
  int result = vsscanf(input, format, ap);
  va_end(ap);
  return result;
}

__attribute__((visibility("default"))) int scanf_fixture(void) {
  signed char hh = 0; short h = 0; int i = 0; long l = 0;
  long long ll = 0; intmax_t j = 0; size_t z = 0; ptrdiff_t t = 0;
  if (sscanf("-1 2 3 4 5 6 7 -8", "%hhd %hd %d %ld %lld %jd %zd %td",
             &hh, &h, &i, &l, &ll, &j, &z, &t) != 8 || hh != -1 || h != 2 ||
      i != 3 || l != 4 || ll != 5 || j != 6 || z != 7 || t != -8) return 1;

  unsigned u = 0, x = 0, o = 0, b = 0; void* p = 0;
  if (sscanf("17 ff 20 0b101 0x1234", "%u %x %o %b %p", &u, &x, &o, &b, &p) != 5 ||
      u != 17 || x != 255 || o != 16 || b != 5 || (uintptr_t)p != 0x1234) return 2;

  char word[8] = {0}, chars[4] = {0}, set[8] = {0}; int count = -1;
  if (sscanf("  abc qwe XYZ-42", "%3s %3c %3[A-Z]-%*d%n", word, chars, set, &count) != 3 ||
      word[0] != 'a' || word[3] != 0 || chars[0] != 'q' || chars[2] != 'e' ||
      set[0] != 'X' || set[3] != 0 || count != 16) return 3;

  float f = 0; double d = 0; Binary128 q = {0};
  if (sscanf("1.25 2.5 1.5", "%f %lf %Lf", &f, &d, &q.value) != 3 ||
      f != 1.25f || d != 2.5 || q.word[0] != 0 || q.word[1] != UINT64_C(0x3fff800000000000)) return 4;

  int a=0,c=0,e=0,g=0,k=0,m=0,o2=0,q2=0;
  if (mixed("1 2 3 4 5 6 7 8", "%d %d %d %d %d %d %d %d",
            1.0, &a, 2.0, &c, 3.0, &e, 4.0, &g,
            5.0, &k, 6.0, &m, 7.0, &o2, 8.0, &q2) != 8 ||
      a!=1 || c!=2 || e!=3 || g!=4 || k!=5 || m!=6 || o2!=7 || q2!=8) return 5;

  long long qext=0, wf16=0; signed char w8=0; short w16=0; int w32=0; long long w64=0;
  if (sscanf("9 10 11 12 13 14", "%qd %w8d %w16d %w32d %w64d %wf16d",
             &qext, &w8, &w16, &w32, &w64, &wf16) != 6 || qext!=9 || w8!=10 ||
      w16!=11 || w32!=12 || w64!=13 || wf16!=14) return 6;
  uint32_t wide[4] = {0};
  if (sscanf("\xc3\xa9!", "%2ls", wide) != 1 || wide[0] != UINT32_C(0xe9) ||
      wide[1] != '!' || wide[2] != 0) return 7;

  errno = 0;
  const char* malformed = "%md";
  if (sscanf("1", malformed, &i) != -1 || errno != 95) return 8;
  errno = 0;
  if (sscanf("999999999999999999999999", "%lld", &ll) != 1 || errno != ERANGE)
    return 9;
  errno = 7;
  if (sscanf("999999999999999999999999 ok", "%*lld %2s", word) != 1 ||
      errno != 7 || word[0] != 'o' || word[1] != 'k') return 10;
  if (sscanf("x", "%d", &i) != 0 || sscanf("", "%d", &i) != -1) return 11;

  FILE* file = fopen("/system/scan.txt", "r");
  int file_i = 0, final_i = 0;
  long long file_ll = 0;
  char file_word[5] = {0};
  if (file == NULL ||
      fscanf(file, "%d %lld %4s", &file_i, &file_ll, file_word) != 3 ||
      file_i != 42 || file_ll != INT64_C(9000000000) ||
      file_word[0] != 't' || file_word[3] != 'l')
    return 12;
  if (fscanf(file, "%d", &final_i) != 1 || final_i != 7) return 13;
  if (fclose(file) != 0) return 14;
  return 42;
}
