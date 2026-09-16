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

// Modified by Rook Systems, Inc. in 2026 from rmf_ros2 2.1.8: the node takes a
// recording directory. The whole change is ../../../tap.patch.

#ifndef RMF_TRAFFIC_ROS2__BLOCKADE__NODE_HPP
#define RMF_TRAFFIC_ROS2__BLOCKADE__NODE_HPP

#include <rclcpp/node.hpp>

#include <filesystem>

namespace rmf_traffic_ros2 {
namespace blockade {

/// Make a blockade node instance
std::shared_ptr<rclcpp::Node> make_node(
  const std::filesystem::path& output_directory,
  const rclcpp::NodeOptions& options = rclcpp::NodeOptions());

/// Make a blockade node instance, specifying a node name
std::shared_ptr<rclcpp::Node> make_node(
  const std::string& node_name,
  const std::filesystem::path& output_directory,
  const rclcpp::NodeOptions& options = rclcpp::NodeOptions());

} // namespace blockade
} // namespace rmf_traffic_ros2

#endif // RMF_TRAFFIC_ROS2__BLOCKADE__NODE_HPP
