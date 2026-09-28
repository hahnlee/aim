#include "aim_bionic_allocator.h"
#include "aim_bionic_errno.h"
#include "aim_bionic_format.h"
#include "aim_elf_loader.h"

#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <iterator>
#include <vector>

AimElfResolveStatus Resolve(void*,const AimElfSymbolRequest* request,uintptr_t* address,AimElfErrorBuffer*) {
  if(!request||!address||!request->symbol)return AIM_ELF_RESOLVE_ERROR;
  if(auto f=aim_bionic_format_resolve(request->symbol)){*address=reinterpret_cast<uintptr_t>(f);return AIM_ELF_RESOLVE_FOUND;}
  if(auto f=aim_bionic_allocator_resolve(request->symbol)){*address=reinterpret_cast<uintptr_t>(f);return AIM_ELF_RESOLVE_FOUND;}
  if(auto f=aim_bionic_errno_resolve(request->symbol)){*address=reinterpret_cast<uintptr_t>(f);return AIM_ELF_RESOLVE_FOUND;}
  return AIM_ELF_RESOLVE_ERROR;
}

int main(int argc,char** argv) {
  if(argc!=2)return 2;std::ifstream in(argv[1],std::ios::binary);std::vector<uint8_t> bytes((std::istreambuf_iterator<char>(in)),{});if(bytes.empty())return 3;
  AimElfLoadOptions options{AIM_ELF_ABI_VERSION,&Resolve,nullptr};AimElfHandle* handle=nullptr;char message[512]{};AimElfErrorBuffer error{message,sizeof(message),0};
  if(aim_elf_load_bytes(bytes.data(),bytes.size(),&options,&handle,&error)!=AIM_ELF_OK){std::fprintf(stderr,"load: %s\n",message);return 4;}
  uintptr_t address=0;if(aim_elf_lookup(handle,"format_fixture_run",&address,&error)!=AIM_ELF_OK)return 5;
  int result=reinterpret_cast<int(*)()>(address)();
  if(aim_elf_unload(&handle,&error)!=AIM_ELF_OK)return 6;
  if(result!=42){std::fprintf(stderr,"fixture=%d\n",result);return 7;}
  std::puts("bionic-format-elf: PASS Android-AAPCS64 variadic+va_list relocation/call allocator+errno=owned");return 0;
}
