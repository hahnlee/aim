#include "compat/filesystem/document_panel.h"

char* (*open_document_abi)(const char*) = darwin_art_host_open_document;
char* (*save_document_abi)(const char*, const char*) = darwin_art_host_save_document;
