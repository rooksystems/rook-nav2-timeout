cmake_minimum_required(VERSION 3.14)
project(rook_blockade_tap)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(CMAKE_COMPILER_IS_GNUCXX OR CMAKE_CXX_COMPILER_ID MATCHES "Clang")
  add_compile_options(-Wall -Wextra -Wpedantic)
endif()

find_package(ament_cmake REQUIRED)
find_package(rclcpp REQUIRED)
find_package(rmf_traffic 3 REQUIRED)
find_package(rmf_traffic_msgs REQUIRED)

add_executable(rook_blockade_tap
  src/main.cpp
  src/Node.cpp
  src/Recorder.cpp
)

target_include_directories(rook_blockade_tap PRIVATE include)
ament_target_dependencies(rook_blockade_tap PUBLIC rclcpp rmf_traffic_msgs)
target_link_libraries(rook_blockade_tap PRIVATE rmf_traffic::rmf_traffic)

install(TARGETS rook_blockade_tap
  RUNTIME DESTINATION lib/${PROJECT_NAME}
)

ament_package()
