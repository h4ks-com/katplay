# Builds libchiaki from the chiaki-ng checkout at CHIAKI_PATH and links a probe
# against it, so build.rs reads the link line CMake resolved for it.
cmake_minimum_required(VERSION 3.16)
project(katplay_chiaki C)

set(CHIAKI_PATH "" CACHE PATH "the chiaki-ng checkout")
if(NOT CHIAKI_PATH)
	message(FATAL_ERROR "CHIAKI_PATH names no chiaki-ng checkout")
endif()

set(CHIAKI_ENABLE_GUI OFF CACHE BOOL "" FORCE)
set(CHIAKI_ENABLE_CLI OFF CACHE BOOL "" FORCE)
set(CHIAKI_ENABLE_TESTS OFF CACHE BOOL "" FORCE)
set(CHIAKI_ENABLE_FFMPEG_DECODER ON CACHE BOOL "" FORCE)
set(CHIAKI_ENABLE_PI_DECODER OFF CACHE BOOL "" FORCE)
set(CHIAKI_LIB_ENABLE_OPUS ON CACHE BOOL "" FORCE)
set(CHIAKI_ENABLE_SETSU OFF CACHE BOOL "" FORCE)
set(CHIAKI_ENABLE_SPEEX OFF CACHE BOOL "" FORCE)
set(CHIAKI_ENABLE_STEAMDECK_NATIVE OFF CACHE BOOL "" FORCE)
set(CHIAKI_ENABLE_STEAM_SHORTCUT OFF CACHE BOOL "" FORCE)
set(CHIAKI_GUI_ENABLE_SDL_GAMECONTROLLER OFF CACHE BOOL "" FORCE)

# We link every library statically so the binary runs on a machine with none of them installed.
set(FFMPEG_PREFIX "" CACHE PATH "the static FFmpeg build")
if(NOT FFMPEG_PREFIX)
	message(FATAL_ERROR "FFMPEG_PREFIX names no FFmpeg build")
endif()
set(ENV{PKG_CONFIG_PATH} "${FFMPEG_PREFIX}/lib/pkgconfig:$ENV{PKG_CONFIG_PATH}")
# libchiaki adds the Homebrew include directory for opus, which can hold another FFmpeg's headers.
include_directories(BEFORE "${FFMPEG_PREFIX}/include")
set(CMAKE_FIND_LIBRARY_SUFFIXES .a)
set(PKG_CONFIG_ARGN --static)
set(OPENSSL_USE_STATIC_LIBS ON)
foreach(option USE_NGHTTP2 USE_LIBIDN2 CURL_USE_LIBPSL CURL_USE_LIBSSH2)
	set(${option} OFF CACHE BOOL "" FORCE)
endforeach()
foreach(dependency ZLIB BROTLI ZSTD)
	set(CURL_${dependency} OFF CACHE STRING "" FORCE)
endforeach()

add_subdirectory("${CHIAKI_PATH}" chiaki)

add_executable(katplay_chiaki_probe probe.c)
find_package(PkgConfig REQUIRED)
pkg_check_modules(SWSCALE REQUIRED IMPORTED_TARGET libswscale)
target_link_libraries(katplay_chiaki_probe PRIVATE chiaki-lib PkgConfig::SWSCALE)
