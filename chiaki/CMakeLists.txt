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

add_subdirectory("${CHIAKI_PATH}" chiaki)

add_executable(katplay_chiaki_probe probe.c)
target_link_libraries(katplay_chiaki_probe PRIVATE chiaki-lib)
