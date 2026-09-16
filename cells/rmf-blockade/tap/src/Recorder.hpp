#ifndef ROOK_BLOCKADE_TAP__RECORDER_HPP
#define ROOK_BLOCKADE_TAP__RECORDER_HPP

#include <rmf_traffic_msgs/msg/blockade_cancel.hpp>
#include <rmf_traffic_msgs/msg/blockade_heartbeat.hpp>
#include <rmf_traffic_msgs/msg/blockade_reached.hpp>
#include <rmf_traffic_msgs/msg/blockade_ready.hpp>
#include <rmf_traffic_msgs/msg/blockade_release.hpp>
#include <rmf_traffic_msgs/msg/blockade_set.hpp>

#include <cstdint>
#include <cstdio>
#include <filesystem>
#include <string>

namespace rook_blockade_tap {

class Recorder
{
public:
  explicit Recorder(const std::filesystem::path& output_directory);
  ~Recorder();
  Recorder(const Recorder&) = delete;
  Recorder& operator=(const Recorder&) = delete;

  void write_set(const rmf_traffic_msgs::msg::BlockadeSet& message);
  void write_ready(const rmf_traffic_msgs::msg::BlockadeReady& message);
  void write_reached(const rmf_traffic_msgs::msg::BlockadeReached& message);
  void write_release(const rmf_traffic_msgs::msg::BlockadeRelease& message);
  void write_cancel(const rmf_traffic_msgs::msg::BlockadeCancel& message);
  void write_heartbeat_timer();
  void write_heartbeat(const rmf_traffic_msgs::msg::BlockadeHeartbeat& message);

private:
  std::uint64_t next_tick();
  void write_delivery(
    std::uint64_t tick,
    std::uint32_t channel,
    const std::string& payload);

  std::FILE* deliveries = nullptr;
  std::FILE* heartbeats = nullptr;
  std::uint64_t last_tick = 0;
};

} // namespace rook_blockade_tap

#endif // ROOK_BLOCKADE_TAP__RECORDER_HPP
