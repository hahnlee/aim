#include <stdarg.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>

static int same(const char* a, const char* b) { while (*a && *a == *b) { ++a; ++b; } return *a == *b; }
static int via_vsnprintf(char* out, size_t size, const char* format, ...) {
  va_list ap; va_start(ap, format); int result=vsnprintf(out,size,format,ap); va_end(ap); return result;
}
static int via_vasprintf(char** out, const char* format, ...) {
  va_list ap; va_start(ap, format); int result=vasprintf(out,format,ap); va_end(ap); return result;
}

__attribute__((visibility("default"))) int format_fixture_run(void) {
  char first[128];
  int n=snprintf(first,sizeof(first),"%+06d|%#x|%.3s|%.2f|%p",-42,0x2a,"hello",3.5,(void*)(uintptr_t)0x1234);
  if(n!=27 || !same(first,"-00042|0x2a|hel|3.50|0x1234")) return 10;
  char second[128];
  n=via_vsnprintf(second,sizeof(second),"%lld/%u/%s/%.1e",(long long)-9000000000LL,17u,"ok",12.5);
  if(n!=25 || !same(second,"-9000000000/17/ok/1.2e+01")) return 11;
  char tiny[5]; n=snprintf(tiny,sizeof(tiny),"abcdef");
  if(n!=6 || !same(tiny,"abcd")) return 12;
  char* allocated=0; n=via_vasprintf(&allocated,"%08x:%s:%.1f",0x2a,"owned",2.25);
  if(n!=18 || allocated==0 || !same(allocated,"0000002a:owned:2.2")) return 13;
  free(allocated);
  const char rejected[]={'b','a','d','%','n',0};
  int untouched=77; errno=0; n=snprintf(first,sizeof(first),rejected,&untouched);
  if(n!=-1 || untouched!=77 || errno!=95) return 14;
  return 42;
}
