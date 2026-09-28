#include "aim_bionic_format.h"
#include "aim_bionic_errno.h"

#include <cstdio>
#include <cstring>
#include <limits>

extern "C" int aim_bionic_snprintf_captured(char*,size_t,const char*,const uint64_t*,const uint8_t*,uint8_t*,uint32_t);

static bool Check(const char* expected,const char* format,uint64_t a=0,uint64_t b=0,uint64_t c=0,double fp0=0.0) {
  alignas(16) uint64_t gp[8]={0,0,0,a,b,c,0,0};alignas(16) uint8_t fp[128]{};std::memcpy(fp,&fp0,8);alignas(16) uint8_t stack[32]{};char out[256];
  int n=aim_bionic_snprintf_captured(out,sizeof(out),format,gp,fp,stack,3);return n==int(std::strlen(expected))&&std::strcmp(out,expected)==0;
}

int main() {
  struct IntegerCase { const char* expected; const char* format; uint64_t value; };
  const IntegerCase integers[]={
    {"a1234b","a%db",1234},{"a-8123b","a%db",uint64_t(int64_t(-8123))},{"a16b","a%hdb",0x7fff0010},
    {"a16b","a%hhdb",0x7fffff10},{"a68719476736b","a%lldb",0x1000000000ULL},{"a70000b","a%ldb",70000},
    {"a12abz","a%xz",0x12ab},{"a12ABz","a%Xz",0x12ab},{"a00123456z","a%08xz",0x123456},
    {"a 1234z","a%5dz",1234},{"a01234z","a%05dz",1234},{"a    1234z","a%8dz",1234},
    {"a1234    z","a%-8dz",1234},{"9223372036854775807","%jd",uint64_t(INT64_MAX)},
    {"-9223372036854775808","%lld",uint64_t(INT64_MIN)},{"18446744073709551615","%ju",UINT64_MAX},
    {"37777777777","%o",UINT32_MAX},{"4294967295","%u",UINT32_MAX},{"ffffffff","%x",UINT32_MAX},
    {"FFFFFFFF","%X",UINT32_MAX},{"a0x0z","a%pz",0},{"a0xb0001234b","a%pb",0xb0001234},
  };
  for(const auto& test:integers)if(!Check(test.expected,test.format,test.value))return 1;
  if(!Check("a01234b","a%sb",reinterpret_cast<uint64_t>("01234"))||
     !Check("a(null)b","a%sb",0)||!Check("abc","a%cc",'b')||
     !Check("Aabcdef     Z","A%-11sZ",reinterpret_cast<uint64_t>("abcdef")))return 2;
  if(!Check("a_1.230000_b","a_%f_b",0,0,0,1.23)||!Check("a_3.14_b","a_%g_b",0,0,0,3.14)||
     !Check("1.500000e+00","%e",0,0,0,1.5)||!Check("-0.000000E+00","%E",0,0,0,-0.0)||
     !Check("-0.000000","%F",0,0,0,-0.0)||!Check("-0","%G",0,0,0,-0.0))return 3;
  alignas(16) uint64_t gp[8]{};alignas(16) uint8_t fp[128]{};alignas(16) uint8_t stack[32]{};char output[8];
  aim_bionic_errno_store(0);int n=aim_bionic_snprintf_captured(output,sizeof(output),"%n",gp,fp,stack,3);
  if(n!=-1||aim_bionic_errno_load()!=95)return 4;
  std::puts("bionic-format-differential: PASS AOSP-stdio-corpus=32 integer+string+pointer+double truncation fail-closed ASan+UBSan");return 0;
}
