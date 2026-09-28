#include <bits/struct_file.h>
#include <stddef.h>
#include <stdio.h>
_Static_assert(sizeof(FILE)==152&&_Alignof(FILE)==8,"Android FILE ABI drift");
typedef FILE*(*Fopen)(const char*,const char*);typedef int(*Fclose)(FILE*);typedef int(*Fflush)(FILE*);typedef int(*Fileno)(FILE*);
typedef size_t(*Fread)(void*,size_t,size_t,FILE*);typedef size_t(*Fwrite)(const void*,size_t,size_t,FILE*);
typedef int(*Fseek)(FILE*,long,int);typedef int(*Fseeko)(FILE*,off_t,int);typedef off_t(*Ftello)(FILE*);typedef int(*Fputc)(int,FILE*);typedef int(*Getc)(FILE*);typedef int(*Ungetc)(int,FILE*);
typedef int(*Fputs)(const char*,FILE*);
typedef void(*Setbuf)(FILE*,char*);
typedef int(*Ferror)(FILE*);typedef long(*Ftell)(FILE*);typedef void(*Perror)(const char*);
_Static_assert(_Generic(&fopen,Fopen:1,default:0),"fopen");_Static_assert(_Generic(&fclose,Fclose:1,default:0),"fclose");
_Static_assert(_Generic(&fflush,Fflush:1,default:0),"fflush");_Static_assert(_Generic(&fileno,Fileno:1,default:0),"fileno");
_Static_assert(_Generic(&fread,Fread:1,default:0),"fread");_Static_assert(_Generic(&fwrite,Fwrite:1,default:0),"fwrite");
_Static_assert(_Generic(&fseek,Fseek:1,default:0),"fseek");_Static_assert(_Generic(&fseeko,Fseeko:1,default:0),"fseeko");
_Static_assert(_Generic(&ftello,Ftello:1,default:0),"ftello");_Static_assert(_Generic(&fputc,Fputc:1,default:0),"fputc");
_Static_assert(_Generic(&fputs,Fputs:1,default:0),"fputs");
_Static_assert(_Generic(&setbuf,Setbuf:1,default:0),"setbuf");
_Static_assert(_Generic(&ferror,Ferror:1,default:0),"ferror");_Static_assert(_Generic(&ftell,Ftell:1,default:0),"ftell");
_Static_assert(_Generic(&perror,Perror:1,default:0),"perror");
_Static_assert(_Generic(&getc,Getc:1,default:0),"getc");_Static_assert(_Generic(&ungetc,Ungetc:1,default:0),"ungetc");
