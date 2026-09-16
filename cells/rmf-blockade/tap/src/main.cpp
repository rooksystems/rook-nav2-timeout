/*
 * Copyright (C) 2020 Open Source Robotics Foundation
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 *
*/

// Modified by Rook Systems, Inc. in 2026 from rmf_ros2 2.1.8: passes the
// recording directory and runs a single-threaded executor. The whole change
// is ../tap.patch.

#include <rmf_traffic_ros2/blockade/Node.hpp>

#include <rclcpp/executors/single_threaded_executor.hpp>
#include <rclcpp/rclcpp.hpp>
#include <rclcpp/utilities.hpp>

#include <fcntl.h>
#include <unistd.h>

#include <cerrno>
#include <cstdio>
#include <cstring>
#include <cstdlib>
#include <filesystem>
#include <iostream>
#include <stdexcept>
#include <string>
#include <vector>

namespace {

constexpr const char* min_conflict_angle_radians = "0.2617993877991494";

std::filesystem::path output_directory(const std::vector<std::string>& args)
{
  if (args.size() == 2)
    return args[1];

  if (args.size() > 2)
    throw std::runtime_error("expected at most one output-directory argument");

  const char* const env = std::getenv("ROOK_OUTPUT_DIR");
  if (env != nullptr && env[0] != '\0')
    return env;

  throw std::runtime_error(
    "output directory required as one CLI argument or ROOK_OUTPUT_DIR");
}

void write_params(const std::filesystem::path& output)
{
  std::filesystem::create_directories(output);
  const auto path = output / "params";
  const int fd = ::open(path.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666);
  if (fd < 0)
  {
    if (errno == EEXIST)
      throw std::runtime_error("Rook recording file already exists: " + path.string());

    throw std::runtime_error(
      "failed to exclusively create params " + path.string()
      + ": " + std::strerror(errno));
  }
  std::FILE* params = ::fdopen(fd, "w");
  if (params == nullptr)
  {
    const int error = errno;
    ::close(fd);
    throw std::runtime_error(
      "failed to open new params file " + path.string()
      + ": " + std::strerror(error));
  }

  const std::string contents =
    "# Recorded configuration of the rmf_traffic_ros2 blockade node.\n"
    "# 15 degrees, the make_node override in rmf_ros2 2.1.8.\n"
    "min_conflict_angle_radians = " + std::string(min_conflict_angle_radians) + "\n";
  if (std::fwrite(contents.data(), 1, contents.size(), params) != contents.size()
      || std::fflush(params) != 0)
  {
    std::fclose(params);
    throw std::runtime_error("failed to write and flush params");
  }
  if (std::fclose(params) != 0)
    throw std::runtime_error("failed to close params");
}

} // namespace

int main(int argc, char* argv[])
{
  try
  {
    const auto non_ros_args = rclcpp::init_and_remove_ros_arguments(argc, argv);
    const auto output = output_directory(non_ros_args);
    write_params(output);

    const auto node = rmf_traffic_ros2::blockade::make_node(output);

    RCLCPP_INFO(
      node->get_logger(),
      "Beginning traffic blockade node with Rook callback tap");

    // Invariant: the recording order equals callback processing order only
    // while this node runs in a SingleThreadedExecutor.
    rclcpp::executors::SingleThreadedExecutor executor;
    executor.add_node(node);
    executor.spin();

    RCLCPP_INFO(
      node->get_logger(),
      "Closing down traffic blockade node with Rook callback tap");

    rclcpp::shutdown();
    return 0;
  }
  catch (const std::exception& error)
  {
    std::cerr << "rook_blockade_tap: " << error.what() << '\n';
    rclcpp::shutdown();
    return 1;
  }
}
