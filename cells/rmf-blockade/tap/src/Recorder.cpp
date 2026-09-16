#include "Recorder.hpp"

#include <fcntl.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <chrono>
#include <cstring>
#include <filesystem>
#include <limits>
#include <stdexcept>
#include <type_traits>
#include <vector>

namespace {

static_assert(std::numeric_limits<double>::is_iec559, "Rook requires IEEE-754 doubles");
static_assert(sizeof(double) == sizeof(std::uint64_t), "Rook requires 64-bit doubles");

template<typename Integer>
void append_integer(std::string& output, Integer value)
{
  static_assert(std::is_integral<Integer>::value, "integer type required");
  using Unsigned = typename std::make_unsigned<Integer>::type;
  const auto bits = static_cast<Unsigned>(value);
  for (std::size_t i = 0; i < sizeof(Integer); ++i)
    output.push_back(static_cast<char>((bits >> (8U*i)) & 0xffU));
}

void append_double(std::string& output, double value)
{
  std::uint64_t bits = 0;
  std::memcpy(&bits, &value, sizeof(bits));
  append_integer(output, bits);
}

void write_all(std::FILE* stream, const std::string& bytes)
{
  if (std::fwrite(bytes.data(), 1, bytes.size(), stream) != bytes.size()
      || std::fflush(stream) != 0)
    throw std::runtime_error("failed to write and flush Rook recording");
}

std::FILE* open_exclusive(const std::filesystem::path& path)
{
  const int fd = ::open(path.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666);
  if (fd < 0)
  {
    if (errno == EEXIST)
      throw std::runtime_error("Rook recording file already exists: " + path.string());

    throw std::runtime_error(
      "failed to exclusively create Rook recording file " + path.string()
      + ": " + std::strerror(errno));
  }

  std::FILE* stream = ::fdopen(fd, "wb");
  if (stream != nullptr)
    return stream;

  const int error = errno;
  ::close(fd);
  throw std::runtime_error(
    "failed to open new Rook recording file " + path.string()
    + ": " + std::strerror(error));
}

std::uint64_t wall_time_ns()
{
  const auto now = std::chrono::system_clock::now().time_since_epoch();
  const auto ns = std::chrono::duration_cast<std::chrono::nanoseconds>(now).count();
  if (ns < 0)
    throw std::runtime_error("system clock precedes Unix epoch");

  return static_cast<std::uint64_t>(ns);
}

std::string encode_triple(
  std::uint64_t participant,
  std::uint64_t reservation,
  std::uint64_t checkpoint)
{
  std::string payload;
  payload.reserve(24);
  append_integer(payload, participant);
  append_integer(payload, reservation);
  append_integer(payload, checkpoint);
  return payload;
}

std::uint32_t checked_u32(std::size_t value, const char* field)
{
  if (value > std::numeric_limits<std::uint32_t>::max())
    throw std::length_error(std::string(field) + " exceeds uint32 wire limit");

  return static_cast<std::uint32_t>(value);
}

std::uint16_t checked_u16(std::size_t value, const char* field)
{
  if (value > std::numeric_limits<std::uint16_t>::max())
    throw std::length_error(std::string(field) + " exceeds uint16 wire limit");

  return static_cast<std::uint16_t>(value);
}

} // namespace

namespace rook_blockade_tap {

Recorder::Recorder(const std::filesystem::path& output_directory)
{
  std::filesystem::create_directories(output_directory);
  deliveries = open_exclusive(output_directory / "deliveries.bin");
  try
  {
    heartbeats = open_exclusive(output_directory / "heartbeats_recorded.bin");
  }
  catch (...)
  {
    std::fclose(deliveries);
    deliveries = nullptr;
    throw;
  }
}

Recorder::~Recorder()
{
  if (heartbeats != nullptr)
    std::fclose(heartbeats);
  if (deliveries != nullptr)
    std::fclose(deliveries);
}

std::uint64_t Recorder::next_tick()
{
  const auto now = wall_time_ns();
  if (last_tick == std::numeric_limits<std::uint64_t>::max())
    throw std::overflow_error("Rook recording tick overflow");

  last_tick = std::max(now, last_tick + 1);
  return last_tick;
}

void Recorder::write_delivery(
  std::uint64_t tick,
  std::uint32_t channel,
  const std::string& payload)
{
  std::string record;
  record.reserve(16 + payload.size());
  append_integer(record, tick);
  append_integer(record, channel);
  append_integer(record, checked_u32(payload.size(), "delivery payload"));
  record.append(payload);
  write_all(deliveries, record);
}

void Recorder::write_set(
  const rmf_traffic_msgs::msg::BlockadeSet& message)
{
  const auto tick = next_tick();
  std::string payload;
  append_integer(payload, message.participant);
  append_integer(payload, message.reservation);
  append_double(payload, message.radius);
  append_integer(payload, checked_u32(message.path.size(), "blockade path"));
  for (const auto& checkpoint : message.path)
  {
    append_double(payload, checkpoint.position[0]);
    append_double(payload, checkpoint.position[1]);
    append_integer<std::uint8_t>(payload, checkpoint.can_hold ? 1 : 0);
    append_integer(
      payload,
      checked_u16(checkpoint.map_name.size(), "checkpoint map name"));
    payload.append(checkpoint.map_name);
  }

  write_delivery(tick, 10, payload);
}

void Recorder::write_ready(
  const rmf_traffic_msgs::msg::BlockadeReady& message)
{
  const auto tick = next_tick();
  write_delivery(
    tick,
    11,
    encode_triple(message.participant, message.reservation, message.checkpoint));
}

void Recorder::write_reached(
  const rmf_traffic_msgs::msg::BlockadeReached& message)
{
  const auto tick = next_tick();
  write_delivery(
    tick,
    12,
    encode_triple(message.participant, message.reservation, message.checkpoint));
}

void Recorder::write_release(
  const rmf_traffic_msgs::msg::BlockadeRelease& message)
{
  const auto tick = next_tick();
  write_delivery(
    tick,
    13,
    encode_triple(message.participant, message.reservation, message.checkpoint));
}

void Recorder::write_cancel(
  const rmf_traffic_msgs::msg::BlockadeCancel& message)
{
  const auto tick = next_tick();
  std::string payload;
  payload.reserve(17);
  append_integer(payload, message.participant);
  append_integer<std::uint8_t>(payload, message.all_reservations ? 1 : 0);
  append_integer(payload, message.reservation);
  write_delivery(tick, 14, payload);
}

void Recorder::write_heartbeat_timer()
{
  write_delivery(next_tick(), 15, {});
}

void Recorder::write_heartbeat(
  const rmf_traffic_msgs::msg::BlockadeHeartbeat& message)
{
  const auto tick = next_tick();
  std::vector<rmf_traffic_msgs::msg::BlockadeStatus> statuses = message.statuses;
  std::sort(
    statuses.begin(), statuses.end(),
    [](const auto& lhs, const auto& rhs)
    {
      return lhs.participant < rhs.participant;
    });

  std::string payload;
  payload.reserve(5 + 49*statuses.size());
  append_integer<std::uint8_t>(payload, message.has_gridlock ? 1 : 0);
  append_integer(payload, checked_u32(statuses.size(), "heartbeat statuses"));
  for (const auto& status : statuses)
  {
    append_integer(payload, status.participant);
    append_integer(payload, status.reservation);
    append_integer<std::uint8_t>(payload, status.any_ready ? 1 : 0);
    append_integer(payload, status.last_ready);
    append_integer(payload, status.last_reached);
    append_integer(payload, status.assignment_begin);
    append_integer(payload, status.assignment_end);
  }

  std::string record;
  record.reserve(12 + payload.size());
  append_integer(record, tick);
  append_integer(record, checked_u32(payload.size(), "heartbeat payload"));
  record.append(payload);
  write_all(heartbeats, record);
}

} // namespace rook_blockade_tap
