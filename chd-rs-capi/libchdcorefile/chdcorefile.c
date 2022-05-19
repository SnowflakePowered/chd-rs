#include <stdio.h>
#include "chdcorefile.h"

core_file* core_fopen(const char* filename) {
    FILE* f = fopen(filename, "rb");
    return (core_file*)f;
}
size_t core_fread(core_file* file, void* buffer, size_t size) {
    FILE* f = (FILE*)file;
    return fread(buffer, 1, size, f);
}

int core_fseek(core_file* file, size_t offset, int origin) {
    FILE* f = (FILE*)file;
    return fseek(f, offset, origin);
}
void core_fclose(core_file* file) {
    FILE* f = (FILE*)file;
    fclose(f);
}
