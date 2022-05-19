cmake_minimum_required(VERSION 3.12 FATAL_ERROR)
project(chdcorefile)

set(CMAKE_CXX_STANDARD 17)
set(BUILD_SHARED_LIBS OFF)
set(CMAKE_OBJECT_PATH_MAX 128)

add_library(chdcorefile
        STATIC
        chdcorefile.c chdcorefile.h)

target_link_libraries(chdcorefile -static)