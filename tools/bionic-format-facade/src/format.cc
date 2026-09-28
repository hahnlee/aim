#include "aim_bionic_format.h"

#include "aim_bionic_allocator.h"
#include "aim_bionic_errno.h"

#include <charconv>
#include <climits>
#include <cmath>
#include <cstdint>
#include <cstring>
#include <limits>
#include <string>

namespace {
constexpr int kEinval = 22;
constexpr int kEoverflow = 75;
constexpr int kEnotsup = 95;

struct AndroidVaList { void* stack; void* gr_top; void* vr_top; int32_t gr_offs; int32_t vr_offs; };
struct Cursor { uint8_t* stack; uint8_t* gr_top; uint8_t* vr_top; int32_t gr_offs; int32_t vr_offs; };
struct Spec {
  bool left=false, plus=false, space=false, zero=false, alt=false;
  int width=0, precision=-1;
  enum Length { kNone, kHh, kH, kL, kLl, kZ, kT, kJ } length=kNone;
  char conversion=0;
};
struct Sink {
  char* dst; size_t capacity; size_t count=0;
  void Put(char c) { if (capacity != 0 && count + 1 < capacity) dst[count] = c; ++count; }
  void Text(const char* p, size_t n) { for (size_t i=0;i<n;++i) Put(p[i]); }
  void Finish() { if (capacity != 0) dst[count < capacity ? count : capacity-1] = 0; }
};

void Fail(int value) { aim_bionic_errno_store(value); }
uint8_t* Align8(uint8_t* p) { return reinterpret_cast<uint8_t*>((reinterpret_cast<uintptr_t>(p)+7)&~uintptr_t(7)); }

uint64_t Gp(Cursor* c) {
  if (c->gr_offs < 0) { uint8_t* p=c->gr_top+c->gr_offs; c->gr_offs+=8; uint64_t v; std::memcpy(&v,p,8); return v; }
  c->stack=Align8(c->stack); uint64_t v; std::memcpy(&v,c->stack,8); c->stack+=8; return v;
}
double Fp(Cursor* c) {
  if (c->vr_offs < 0) { uint8_t* p=c->vr_top+c->vr_offs; c->vr_offs+=16; double v; std::memcpy(&v,p,8); return v; }
  c->stack=Align8(c->stack); double v; std::memcpy(&v,c->stack,8); c->stack+=8; return v;
}

bool Parse(const char*& p, Spec* s) {
  for (;;++p) { if (*p=='-') s->left=true; else if (*p=='+') s->plus=true; else if (*p==' ') s->space=true;
    else if (*p=='0') s->zero=true; else if (*p=='#') s->alt=true; else break; }
  if (*p=='*') { s->width=-2; ++p; } else while (*p>='0'&&*p<='9') { if (s->width>1000000) return false; s->width=s->width*10+(*p++-'0'); }
  if (*p=='$') return false;
  if (*p=='.') { ++p; s->precision=0; if (*p=='*') { s->precision=-2; ++p; } else while (*p>='0'&&*p<='9') { if(s->precision>1000000)return false; s->precision=s->precision*10+(*p++-'0'); } if (*p=='$') return false; }
  if (*p=='h') { ++p; s->length=*p=='h'?(++p,Spec::kHh):Spec::kH; }
  else if (*p=='l') { ++p; s->length=*p=='l'?(++p,Spec::kLl):Spec::kL; }
  else if (*p=='z') { ++p; s->length=Spec::kZ; } else if (*p=='t') { ++p; s->length=Spec::kT; }
  else if (*p=='j') { ++p; s->length=Spec::kJ; } else if (*p=='L') return false;
  s->conversion=*p; if (*p) ++p;
  const char* allowed="diuoxXcspfFeEgG%";
  if (s->conversion==0 || std::strchr(allowed,s->conversion)==nullptr) return false;
  if (s->conversion=='n') return false;
  if ((s->conversion=='c'||s->conversion=='s') && s->length!=Spec::kNone) return false;
  if ((s->conversion=='p'||s->conversion=='%') && s->length!=Spec::kNone) return false;
  if (std::strchr("fFeEgG",s->conversion) && s->length!=Spec::kNone && s->length!=Spec::kL) return false;
  if (std::strchr("fFeEgG",s->conversion) && s->alt) return false;
  return true;
}

bool Validate(const char* f) {
  if (!f) return false;
  for (const char* p=f; *p;) { if (*p++!='%') continue; if (*p=='%') {++p;continue;} Spec s; if(!Parse(p,&s))return false; }
  return true;
}

void Padded(Sink* out, const std::string& text, const Spec& s, bool numeric) {
  size_t pad=s.width>0&&static_cast<size_t>(s.width)>text.size()?static_cast<size_t>(s.width)-text.size():0;
  if (!s.left && !(s.zero&&numeric)) while(pad--) out->Put(' ');
  if (!s.left && s.zero&&numeric && pad) {
    size_t prefix=(!text.empty()&&(text[0]=='-'||text[0]=='+'||text[0]==' '))?1:0;
    if (text.size()>=prefix+2&&text[prefix]=='0'&&(text[prefix+1]=='x'||text[prefix+1]=='X')) prefix+=2;
    out->Text(text.data(),prefix); while(pad--)out->Put('0'); out->Text(text.data()+prefix,text.size()-prefix);
  } else { out->Text(text.data(),text.size()); if(s.left)while(pad--)out->Put(' '); }
}

uint64_t UnsignedArg(Cursor* c, Spec::Length l) { uint64_t v=Gp(c); if(l==Spec::kHh)return uint8_t(v); if(l==Spec::kH)return uint16_t(v); if(l==Spec::kNone)return uint32_t(v); return v; }
int64_t SignedArg(Cursor* c, Spec::Length l) { uint64_t v=Gp(c); if(l==Spec::kHh)return int8_t(v); if(l==Spec::kH)return int16_t(v); if(l==Spec::kNone)return int32_t(v); return int64_t(v); }

std::string Integer(uint64_t v, bool negative, unsigned base, bool upper, const Spec& s) {
  char buf[96]; auto r=std::to_chars(buf,buf+sizeof(buf),v,base); std::string digits(buf,r.ptr);
  if (s.precision==0&&v==0) digits.clear();
  while(s.precision>0&&digits.size()<static_cast<size_t>(s.precision))digits.insert(digits.begin(),'0');
  if(upper)for(char& c:digits)if(c>='a'&&c<='f')c-=32;
  if(s.alt&&v!=0&&base==16)digits.insert(0,upper?"0X":"0x"); else if(s.alt&&base==8&&(digits.empty()||digits[0]!='0'))digits.insert(digits.begin(),'0');
  if(negative)digits.insert(digits.begin(),'-'); else if(s.plus)digits.insert(digits.begin(),'+'); else if(s.space)digits.insert(digits.begin(),' ');
  return digits;
}

bool DoubleText(double v, const Spec& s, std::string* out) {
  char buf[768]; int precision=s.precision<0?6:s.precision; if((s.conversion=='g'||s.conversion=='G')&&precision==0)precision=1;
  std::chars_format mode=(s.conversion=='f'||s.conversion=='F')?std::chars_format::fixed:(s.conversion=='e'||s.conversion=='E'?std::chars_format::scientific:std::chars_format::general);
  auto r=std::to_chars(buf,buf+sizeof(buf),v,mode,precision); if(r.ec!=std::errc())return false; out->assign(buf,r.ptr);
  if(s.conversion=='F'||s.conversion=='E'||s.conversion=='G')for(char& c:*out)if(c>='a'&&c<='z')c-=32;
  if(!std::signbit(v)){if(s.plus)out->insert(out->begin(),'+');else if(s.space)out->insert(out->begin(),' ');} return true;
}

int Format(Sink* out, const char* format, Cursor cursor) {
  if(!Validate(format)){out->Finish();Fail(kEnotsup);return -1;}
  for(const char* p=format;*p;) {
    if(*p!='%'){out->Put(*p++);continue;} ++p; if(*p=='%'){out->Put('%');++p;continue;}
    Spec s; if(!Parse(p,&s)){out->Finish();Fail(kEnotsup);return -1;}
    if(s.width==-2){int w=int32_t(Gp(&cursor)); if(w==INT_MIN){out->Finish();Fail(kEoverflow);return -1;} if(w<0){s.left=true;w=-w;}s.width=w;}
    if(s.precision==-2){int n=int32_t(Gp(&cursor));s.precision=n<0?-1:n;}
    std::string text; bool numeric=false;
    if(std::strchr("di",s.conversion)){int64_t v=SignedArg(&cursor,s.length);bool neg=v<0;uint64_t mag=neg?uint64_t(-(v+1))+1:uint64_t(v);text=Integer(mag,neg,10,false,s);numeric=true;}
    else if(std::strchr("uoxX",s.conversion)){unsigned base=s.conversion=='o'?8:(s.conversion=='u'?10:16);text=Integer(UnsignedArg(&cursor,s.length),false,base,s.conversion=='X',s);numeric=true;}
    else if(s.conversion=='c'){text.push_back(char(Gp(&cursor)));}
    else if(s.conversion=='s'){const char* q=reinterpret_cast<const char*>(Gp(&cursor));if(!q)q="(null)";size_t n=std::strlen(q);if(s.precision>=0&&n>size_t(s.precision))n=s.precision;text.assign(q,n);}
    else if(s.conversion=='p'){uintptr_t v=Gp(&cursor);Spec t=s;t.alt=false;text="0x"+Integer(v,false,16,false,t);numeric=true;}
    else {if(!DoubleText(Fp(&cursor),s,&text)){out->Finish();Fail(kEoverflow);return -1;}numeric=true;}
    Spec padded=s;if(s.precision>=0&&std::strchr("diuoxX",s.conversion))padded.zero=false;Padded(out,text,padded,numeric);
    if(out->count>size_t(INT_MAX)){out->Finish();Fail(kEoverflow);return -1;}
  }
  out->Finish(); return static_cast<int>(out->count);
}

Cursor FromVa(const void* opaque) { AndroidVaList v{};std::memcpy(&v,opaque,sizeof(v));return {static_cast<uint8_t*>(v.stack),static_cast<uint8_t*>(v.gr_top),static_cast<uint8_t*>(v.vr_top),v.gr_offs,v.vr_offs}; }
}

extern "C" int aim_bionic_snprintf_captured(char* dst,size_t size,const char* format,const uint64_t* gp,const uint8_t* fp,uint8_t* stack,uint32_t gp_index) {
  if((dst==nullptr&&size!=0)||format==nullptr){Fail(kEinval);return -1;} Cursor c{stack,reinterpret_cast<uint8_t*>(const_cast<uint64_t*>(gp))+64,const_cast<uint8_t*>(fp)+128,int32_t(gp_index*8)-64,-128}; Sink s{dst,size};return Format(&s,format,c);
}
extern "C" int aim_bionic_sprintf_captured(char* dst,const char* format,const uint64_t* gp,const uint8_t* fp,uint8_t* stack) {
  if(dst==nullptr||format==nullptr){Fail(kEinval);return -1;} Cursor c{stack,reinterpret_cast<uint8_t*>(const_cast<uint64_t*>(gp))+64,const_cast<uint8_t*>(fp)+128,-48,-128}; Sink s{dst,SIZE_MAX};return Format(&s,format,c);
}
extern "C" int aim_bionic_asprintf_captured(char** output,const char* format,const uint64_t* gp,const uint8_t* fp,uint8_t* stack) {
  AndroidVaList va{stack,reinterpret_cast<uint8_t*>(const_cast<uint64_t*>(gp))+64,const_cast<uint8_t*>(fp)+128,-48,-128};return aim_bionic_vasprintf(output,format,&va);
}
extern "C" int aim_bionic_vsnprintf(char* dst,size_t size,const char* format,const void* ap) {
  if((dst==nullptr&&size!=0)||!format||!ap){Fail(kEinval);return -1;} Sink s{dst,size};return Format(&s,format,FromVa(ap));
}
extern "C" int aim_bionic_vasprintf(char** output,const char* format,const void* ap) {
  if(!output||!format||!ap){Fail(kEinval);return -1;} *output=nullptr; Sink count{nullptr,0};int n=Format(&count,format,FromVa(ap));if(n<0)return -1;
  AimBionicAllocationResult a=aim_bionic_malloc_result(size_t(n)+1);if(!a.pointer){Fail(a.bionic_errno);return -1;} Sink sink{static_cast<char*>(a.pointer),size_t(n)+1};int written=Format(&sink,format,FromVa(ap));if(written<0){aim_bionic_free(a.pointer);return -1;}*output=static_cast<char*>(a.pointer);return written;
}
extern "C" int aim_bionic___vsnprintf_chk(char* dst,size_t size,int flags,size_t destination_size,const char* format,const void* ap) {
  (void)flags;if(size>destination_size){Fail(kEinval);return -1;}return aim_bionic_vsnprintf(dst,size,format,ap);
}
extern "C" int aim_bionic___vsprintf_chk(char* dst,int flags,size_t destination_size,const char* format,const void* ap) {
  (void)flags;
  int result=aim_bionic_vsnprintf(dst,destination_size,format,ap);
  if(result<0||static_cast<size_t>(result)>=destination_size){Fail(kEoverflow);return -1;}
  return result;
}
extern "C" int aim_bionic_vsprintf(char* dst,const char* format,const void* ap) {
  return aim_bionic_vsnprintf(dst,SIZE_MAX,format,ap);
}
extern "C" AimBionicFormatFunction aim_bionic_format_resolve(const char* s) {
  if(!s)return nullptr;if(std::strcmp(s,"__vsnprintf_chk")==0)return reinterpret_cast<AimBionicFormatFunction>(aim_bionic___vsnprintf_chk);
  if(std::strcmp(s,"__vsprintf_chk")==0)return reinterpret_cast<AimBionicFormatFunction>(aim_bionic___vsprintf_chk);
  if(std::strcmp(s,"asprintf")==0)return reinterpret_cast<AimBionicFormatFunction>(aim_bionic_asprintf);
  if(std::strcmp(s,"snprintf")==0)return reinterpret_cast<AimBionicFormatFunction>(aim_bionic_snprintf);
  if(std::strcmp(s,"sprintf")==0)return reinterpret_cast<AimBionicFormatFunction>(aim_bionic_sprintf);
  if(std::strcmp(s,"vasprintf")==0)return reinterpret_cast<AimBionicFormatFunction>(aim_bionic_vasprintf);
  if(std::strcmp(s,"vsnprintf")==0)return reinterpret_cast<AimBionicFormatFunction>(aim_bionic_vsnprintf);
  if(std::strcmp(s,"vsprintf")==0)return reinterpret_cast<AimBionicFormatFunction>(aim_bionic_vsprintf);
  return nullptr;
}
extern "C" const char* aim_bionic_format_capability(const char* s) {
  if(!s)return "invalid-symbol";if(std::strcmp(s,"fprintf")==0||std::strcmp(s,"vfprintf")==0)return "Bionic-FILE-layout-not-owned";
  if(std::strcmp(s,"sscanf")==0||std::strcmp(s,"vsscanf")==0)return "scan-pointer-write-grammar-not-owned";return aim_bionic_format_resolve(s)?"supported":"unknown-symbol";
}
