// The guard here is __CORETYPES_H__ because this is mutually exclusive with libchdr/coretypes.h
#ifndef __CORETYPES_H__
#define __CORETYPES_H__

typedef void core_file;

size_t core_fread(core_file* file, void* buffer, size_t size);
int core_fseek(core_file* file, size_t offset, int origin);
core_file* core_fopen(const char* filename);
void core_fclose(core_file* file);

#endif //__CORETYPES_H__
