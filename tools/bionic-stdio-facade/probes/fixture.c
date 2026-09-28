#include <bits/struct_file.h>
#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <wchar.h>

extern FILE __sF[];
static FILE* gRace;
static FILE* gWideRace;
static int Equal(const unsigned char* a,const unsigned char* b,size_t n){for(size_t i=0;i<n;i++)if(a[i]!=b[i])return 0;return 1;}

__attribute__((visibility("default"))) int bionic_stdio_fixture_basic(void){
 FILE* in=&__sF[0];FILE* out=&__sF[1];FILE* err=&__sF[2];
 if(stdin!=in||stdout!=out||stderr!=err)return 36;
 setbuf(err,NULL);
 if(fileno(in)!=0||fileno(out)!=1||fileno(err)!=2)return 1;
 if(getc(in)!='i'||ungetc('Z',in)!='Z'||getc(in)!='Z'||getc(in)!='n')return 2;
 if(fputs("ok",err)<0||fputc('!',out)!='!'||ferror(out)!=0||ftell(out)!=1||fflush(out)!=0||fflush(NULL)!=0)return 3;
 FILE* cloexec=fopen("/system/input.bin","re");if(cloexec==NULL)return 37;
 char line[8]={0};
 if(fgetc(cloexec)!='a'||fgets(line,(int)sizeof(line),cloexec)!=line||
    !Equal((const unsigned char*)line,(const unsigned char*)"bcdef",6)||
    fclose(cloexec)!=0)return 38;
 FILE* f=fopen("/system/input.bin","rb");if(f==NULL||fileno(f)<20000)return 4;
 unsigned char b[8]={0};if(fread(b,2,2,f)!=2||!Equal(b,(const unsigned char*)"abcd",4)||ftello(f)!=4)return 5;
 if(getc(f)!='e'||ungetc('Q',f)!='Q'||getc(f)!='Q')return 6;
 if(fseek(f,0,SEEK_SET)!=0||getc(f)!='a')return 7;
 errno=0;if(fwrite("x",1,1,f)!=0||errno!=EBADF||ferror(f)==0)return 8;
 if(fclose(f)!=0)return 9;
 errno=0;if(getc(f)!=EOF||errno!=EBADF)return 10;
 f=fopen("/private/output.bin","w+b");if(f==NULL)return 11;
 if(fwrite("xyz",1,3,f)!=3||ftello(f)!=3||fseeko(f,0,SEEK_SET)!=0)return 12;
 if(fread(b,1,3,f)!=3||!Equal(b,(const unsigned char*)"xyz",3))return 13;
 errno=0;if(fread(b,SIZE_MAX,2,f)!=0||errno!=EOVERFLOW)return 14;
 if(fseeko(f,16*1024*1024,SEEK_SET)!=0)return 15;
 errno=0;if(fputc('x',f)!=EOF||errno!=EFBIG)return 16;
 errno=0;if(fseek(f,0,99)!=-1||errno!=EINVAL)return 17;
 if(fclose(f)!=0)return 18;
 f=fopen("/private/output.bin","r+b");if(f==NULL)return 39;
 if(fseeko(f,0,SEEK_END)!=0||fwrite("!",1,1,f)!=1||fclose(f)!=0)return 40;
 f=fopen("/private/output.bin","rb");if(f==NULL)return 41;
 unsigned char persisted[4]={0};
 if(fread(persisted,1,sizeof(persisted),f)!=sizeof(persisted) ||
    !Equal(persisted,(const unsigned char*)"xyz!",sizeof(persisted)) ||
    fclose(f)!=0)return 42;
 f=fopen("/private/output.bin","a+b");if(f==NULL)return 43;
 if(fseeko(f,0,SEEK_SET)!=0||fwrite("?",1,1,f)!=1||fclose(f)!=0)return 44;
 f=fopen("/private/output.bin","rb");if(f==NULL)return 45;
 unsigned char appended[5]={0};
 if(fread(appended,1,sizeof(appended),f)!=sizeof(appended) ||
    !Equal(appended,(const unsigned char*)"xyz!?",sizeof(appended)) ||
    fclose(f)!=0)return 46;
 errno=0;if(fopen("/system/input.bin","q")!=NULL||errno!=EINVAL)return 19;
 errno=0;if(fopen("","w")!=NULL||errno!=ENOENT)return 20;
 errno=0;if(fopen("/missing","rb")!=NULL||errno!=ENOENT)return 24;
 FILE* wi=fopen("/system/input.bin","rb");if(wi==NULL)return 25;
 if(getwc(wi)!=L'a'||(uint32_t)ungetwc((wint_t)0x1f642,wi)!=0x1f642||
    (uint32_t)getwc(wi)!=0x1f642||getwc(wi)!=L'b')return 26;
 if(fclose(wi)!=0)return 27;
 FILE* wo=fopen("/wide-output.bin","w+b");if(wo==NULL)return 28;
 if((uint32_t)fputwc((wchar_t)0x1f600,wo)!=0x1f600||
    fseeko(wo,0,SEEK_SET)!=0||(uint32_t)getwc(wo)!=0x1f600)return 29;
 if(ungetwc(L'X',wo)!=L'X'||fputwc(L'Y',wo)!=L'Y'||
    fseeko(wo,0,SEEK_SET)!=0||(uint32_t)getwc(wo)!=0x1f600||
    getwc(wo)!=L'Y'||fclose(wo)!=0)return 30;
 return 42;
}
__attribute__((visibility("default"))) int bionic_stdio_fixture_race_setup(void){gRace=fopen("/race","wb");return gRace?42:20;}
__attribute__((visibility("default"))) int bionic_stdio_fixture_race_write(void){int r=fputc('r',gRace);if(r=='r')return 42;if(r==EOF&&errno==EBADF)return 42;return 21;}
__attribute__((visibility("default"))) int bionic_stdio_fixture_race_close(void){return fclose(gRace)==0?42:22;}
__attribute__((visibility("default"))) int bionic_stdio_fixture_race_after(void){errno=0;return fputc('x',gRace)==EOF&&errno==EBADF?42:23;}
__attribute__((visibility("default"))) int bionic_stdio_fixture_wide_race_setup(void){gWideRace=fopen("/system/input.bin","rb");return gWideRace?42:31;}
__attribute__((visibility("default"))) int bionic_stdio_fixture_wide_race_get(void){errno=0;wint_t r=getwc(gWideRace);return r!=WEOF||(errno==0||errno==EBADF)?42:32;}
__attribute__((visibility("default"))) int bionic_stdio_fixture_wide_race_seek(void){errno=0;int r=fseeko(gWideRace,0,SEEK_SET);return r==0||(r==-1&&errno==EBADF)?42:33;}
__attribute__((visibility("default"))) int bionic_stdio_fixture_wide_race_close(void){return fclose(gWideRace)==0?42:34;}
__attribute__((visibility("default"))) int bionic_stdio_fixture_wide_race_after(void){errno=0;return getwc(gWideRace)==WEOF&&errno==EBADF?42:35;}
