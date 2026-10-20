cmake_minimum_required(VERSION 3.16)
project(harmony_probe LANGUAGES C CXX)
if(DEFINED PACKAGE_FIND_FILE)
  include(${PACKAGE_FIND_FILE})
endif()

add_library(harmony_probe SHARED harmony_probe.cpp game_surface.cpp game_resource_import.cpp game_resource_import_core.cpp game_save_import.cpp game_save_import_core.cpp vulkan_buffer_diagnostics.cpp graphics_accel_diagnostics.cpp graphics_spatial_diagnostics.cpp graphics_frame_diagnostics.cpp xengine_spatial_bridge.cpp)
target_compile_features(harmony_probe PRIVATE cxx_std_17)
target_link_libraries(harmony_probe PUBLIC libace_napi.z.so libace_ndk.z.so libnative_window.so libhilog_ndk.z.so libdeviceinfo_ndk.z.so libz.so)
find_path(MUGUET_HMS_NATIVE_INCLUDE xengine/xeg_vulkan_extension.h
  HINTS "${CMAKE_SYSROOT}/../../../hms/native/sysroot/usr/include"
        "/Applications/DevEco-Studio.app/Contents/sdk/default/hms/native/sysroot/usr/include")
if(NOT MUGUET_HMS_NATIVE_INCLUDE)
  message(FATAL_ERROR "Graphics capability diagnostics require the public HMS native SDK headers")
endif()
target_include_directories(harmony_probe PRIVATE "${MUGUET_HMS_NATIVE_INCLUDE}")

# A separate XComponent surface keeps the system-present FG experiment isolated
# from the game renderer and the existing native input bridge.
add_library(graphics_frame_surface SHARED graphics_frame_surface.cpp graphics_frame_scene.cpp)
target_compile_features(graphics_frame_surface PRIVATE cxx_std_17)
target_include_directories(graphics_frame_surface PRIVATE "${MUGUET_HMS_NATIVE_INCLUDE}")
# HMS shared objects are SDK link stubs; resolve the system library on device.
set_target_properties(graphics_frame_surface PROPERTIES SKIP_BUILD_RPATH TRUE)
find_library(MUGUET_FRAME_GENERATION_LIBRARY NAMES framegeneration
  HINTS "${MUGUET_HMS_NATIVE_INCLUDE}/../lib/${CMAKE_LIBRARY_ARCHITECTURE}"
        "${MUGUET_HMS_NATIVE_INCLUDE}/../lib/aarch64-linux-ohos"
  NO_DEFAULT_PATH NO_CMAKE_FIND_ROOT_PATH)
if(NOT MUGUET_FRAME_GENERATION_LIBRARY)
  message(FATAL_ERROR "FG surface probe requires the public HMS framegeneration link library")
endif()
# Keep the public SDK dependency before Vulkan, as in the official integration.
# Direct references in graphics_frame_surface.cpp retain its ELF DT_NEEDED entry.
target_link_libraries(graphics_frame_surface PRIVATE
  libace_napi.z.so libace_ndk.z.so libnative_window.so
  libdeviceinfo_ndk.z.so "${MUGUET_FRAME_GENERATION_LIBRARY}" libvulkan.so dl)

get_filename_component(PROJECT_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../../../.." ABSOLUTE)
target_sources(graphics_frame_surface PRIVATE "${PROJECT_ROOT}/native-diagnostics/src/fg_model_metadata_audit.cpp")
target_sources(graphics_frame_surface PRIVATE "${PROJECT_ROOT}/native-diagnostics/src/fg_npu_flow_benchmark.cpp")
target_sources(graphics_frame_surface PRIVATE "${PROJECT_ROOT}/native-diagnostics/src/fg_model_alias_scope.cpp")
target_sources(graphics_frame_surface PRIVATE "${PROJECT_ROOT}/native-diagnostics/src/fg_rife_benchmark.cpp")
target_sources(graphics_frame_surface PRIVATE "${PROJECT_ROOT}/native-diagnostics/src/fg_rife_stream.cpp" "${PROJECT_ROOT}/native-diagnostics/src/fg_rife_surface.cpp")
target_include_directories(graphics_frame_surface PRIVATE "${PROJECT_ROOT}/native-diagnostics/src")
set(MUGUET_RIFE_SDK "" CACHE PATH "Optional independent OHOS RIFE diagnostic SDK")
if(MUGUET_RIFE_SDK)
  if(NOT EXISTS "${MUGUET_RIFE_SDK}/lib/libharmony_rife_full.a" OR
     NOT EXISTS "${MUGUET_RIFE_SDK}/include/harmony_rife_c.h")
    message(FATAL_ERROR "RIFE diagnostic SDK archive/header missing")
  endif()
  target_include_directories(graphics_frame_surface PRIVATE "${MUGUET_RIFE_SDK}/include")
  target_link_libraries(graphics_frame_surface PRIVATE "${MUGUET_RIFE_SDK}/lib/libharmony_rife_full.a")
  target_compile_definitions(graphics_frame_surface PRIVATE MUGUET_HAS_RIFE=1)
else()
  target_compile_definitions(graphics_frame_surface PRIVATE MUGUET_HAS_RIFE=0)
endif()
option(MUGUET_SHELL_ONLY "Build the public application shell without any game runtime or embedded AOT" OFF)
if(MUGUET_SHELL_ONLY)
  message(STATUS "Public shell mode: game runtime and all embedded AOT objects are explicitly disabled")
endif()
set(GAME_RUNTIME_LIBRARY "${PROJECT_ROOT}/game-runtime/artifacts/libmuguet_game_runtime.a" CACHE FILEPATH "OHOS game runtime static library")
if(NOT MUGUET_SHELL_ONLY AND EXISTS "${GAME_RUNTIME_LIBRARY}")
  add_library(muguet_game_runtime STATIC IMPORTED)
  set_target_properties(muguet_game_runtime PROPERTIES IMPORTED_LOCATION "${GAME_RUNTIME_LIBRARY}")
  target_include_directories(harmony_probe PRIVATE "${PROJECT_ROOT}/game-runtime/include")
  target_link_libraries(harmony_probe PRIVATE muguet_game_runtime libvulkan.so libohaudio.so libqos.so dl m)
  if(MUGUET_RIFE_SDK)
    target_sources(harmony_probe PRIVATE
      "${PROJECT_ROOT}/native-diagnostics/src/fg_npu_flow.cpp"
      "${PROJECT_ROOT}/native-diagnostics/src/fg_npu_rgb.cpp"
      "${PROJECT_ROOT}/native-platform/src/fg_interpolation_bridge.cpp")
    target_include_directories(harmony_probe PRIVATE
      "${PROJECT_ROOT}/native-diagnostics/src"
      "${PROJECT_ROOT}/native-platform/include"
      "${MUGUET_RIFE_SDK}/include")
    set_source_files_properties(
      "${PROJECT_ROOT}/native-diagnostics/src/fg_npu_flow.cpp"
      "${PROJECT_ROOT}/native-diagnostics/src/fg_npu_rgb.cpp"
      "${PROJECT_ROOT}/native-platform/src/fg_interpolation_bridge.cpp"
      PROPERTIES COMPILE_OPTIONS "-O3")
    target_link_libraries(harmony_probe PRIVATE "${MUGUET_RIFE_SDK}/lib/libharmony_rife_full.a")
  endif()
  set_property(TARGET harmony_probe APPEND PROPERTY LINK_DEPENDS "${GAME_RUNTIME_LIBRARY}")
  target_compile_definitions(harmony_probe PRIVATE MG_HAS_GAME_RUNTIME=1)
  set(GAME_AOT_OBJECT "${PROJECT_ROOT}/game-runtime/artifacts/game-embedded.o")
  if(NOT EXISTS "${GAME_AOT_OBJECT}")
    message(FATAL_ERROR "Game runtime exists but its signed game AOT object is missing")
  endif()
  target_sources(harmony_probe PRIVATE "${GAME_AOT_OBJECT}")
  set_property(TARGET harmony_probe APPEND PROPERTY LINK_DEPENDS "${GAME_AOT_OBJECT}")
  if(EXISTS "${PROJECT_ROOT}/native-platform/src/oh_audio.cpp")
    target_sources(harmony_probe PRIVATE "${PROJECT_ROOT}/native-platform/src/oh_audio.cpp")
    target_include_directories(harmony_probe PRIVATE "${PROJECT_ROOT}/native-platform/include")
  endif()
  message(STATUS "Game runtime enabled: ${GAME_RUNTIME_LIBRARY}")
else()
  target_compile_definitions(harmony_probe PRIVATE MG_HAS_GAME_RUNTIME=0)
  message(STATUS "Game runtime is not linked; the game page will report unavailable")
endif()
set(RUST_PROBE_LIBRARY "${PROJECT_ROOT}/runtime-probe/target/aarch64-unknown-linux-ohos/release/libharmony_runtime_probe.a" CACHE FILEPATH "OHOS arm64 Rust static library")
if(NOT MUGUET_SHELL_ONLY AND EXISTS "${RUST_PROBE_LIBRARY}")
  add_library(rust_runtime_probe STATIC IMPORTED)
  set_target_properties(rust_runtime_probe PROPERTIES IMPORTED_LOCATION "${RUST_PROBE_LIBRARY}")
  target_link_libraries(harmony_probe PRIVATE rust_runtime_probe dl m)
  set_property(TARGET harmony_probe APPEND PROPERTY LINK_DEPENDS "${RUST_PROBE_LIBRARY}")
  target_include_directories(harmony_probe PRIVATE "${PROJECT_ROOT}/runtime-probe/include")
  target_compile_definitions(harmony_probe PRIVATE RP_HAS_RUST=1)
  message(STATUS "Rust bridge enabled: ${RUST_PROBE_LIBRARY}")
else()
  target_compile_definitions(harmony_probe PRIVATE RP_HAS_RUST=0)
  message(STATUS "Rust bridge not linked: building C++ baseline only")
endif()

set(EMBEDDED_AOT_OBJECT "${PROJECT_ROOT}/runtime-probe/artifacts/add-embedded.o" CACHE FILEPATH "Trusted AOT artifact embedded in the signed executable mapping")
if(NOT MUGUET_SHELL_ONLY AND EXISTS "${EMBEDDED_AOT_OBJECT}" AND EXISTS "${RUST_PROBE_LIBRARY}")
  target_sources(harmony_probe PRIVATE "${EMBEDDED_AOT_OBJECT}")
  set_property(TARGET harmony_probe APPEND PROPERTY LINK_DEPENDS "${EMBEDDED_AOT_OBJECT}")
  target_compile_definitions(harmony_probe PRIVATE RP_HAS_EMBEDDED_AOT=1)
  message(STATUS "Embedded AOT object enabled: ${EMBEDDED_AOT_OBJECT}")
else()
  target_compile_definitions(harmony_probe PRIVATE RP_HAS_EMBEDDED_AOT=0)
endif()

if(EXISTS "${PROJECT_ROOT}/native-diagnostics/CMakeLists.txt")
  add_subdirectory("${PROJECT_ROOT}/native-diagnostics" "${CMAKE_CURRENT_BINARY_DIR}/native-diagnostics")
  target_link_libraries(harmony_probe PRIVATE muguet_vulkan_probe)
  target_link_libraries(harmony_probe PRIVATE muguet_vulkan_buffer_probe)
  target_compile_definitions(harmony_probe PRIVATE RP_HAS_VULKAN_PROBE=1)
else()
  target_compile_definitions(harmony_probe PRIVATE RP_HAS_VULKAN_PROBE=0)
endif()

function(link_extra_aot name definition)
  set(object "${PROJECT_ROOT}/runtime-probe/artifacts/${name}-embedded.o")
  if(NOT MUGUET_SHELL_ONLY AND EXISTS "${object}" AND EXISTS "${RUST_PROBE_LIBRARY}")
    target_sources(harmony_probe PRIVATE "${object}")
    set_property(TARGET harmony_probe APPEND PROPERTY LINK_DEPENDS "${object}")
    target_compile_definitions(harmony_probe PRIVATE ${definition}=1)
  else()
    target_compile_definitions(harmony_probe PRIVATE ${definition}=0)
  endif()
endfunction()
link_extra_aot("memory64" RP_HAS_MEMORY64_AOT)
link_extra_aot("shared" RP_HAS_SHARED_AOT)
link_extra_aot("shared-memory64" RP_HAS_SHARED_MEMORY64_AOT)
