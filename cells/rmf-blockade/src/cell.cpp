// Rook cell shim around Open-RMF's blockade Moderator.
//
// This file is the entire boundary described in
// docs/internals/rmf-blockade-boundary.md. It decodes the five inbound
// messages, calls the unmodified Moderator, and encodes the heartbeat. The
// vendored rmf_traffic sources are byte-identical to upstream; nothing in
// this file reaches into them.

#include <rmf_traffic/blockade/Moderator.hpp>

#include <algorithm>
#include <cstdint>
#include <cstring>
#include <exception>
#include <string>
#include <vector>

// The cell imports only the ten-function "rook" module.
// Pointer arguments are wasm32 i32; `intptr_t` is that on wasm32 and lets
// native/driver.cpp link the same file against a native implementation of
// the ABI for the native-versus-wasm comparison.
#ifdef __wasm__
#define ROOK_IMPORT(name) __attribute__((import_module("rook"), import_name(#name)))
#else
#define ROOK_IMPORT(name)
#endif
extern "C" {
ROOK_IMPORT(tick) int64_t rook_tick();
ROOK_IMPORT(inbox_len) int32_t rook_inbox_len();
ROOK_IMPORT(inbox_meta) int64_t rook_inbox_meta(int32_t index);
ROOK_IMPORT(inbox_read) int32_t rook_inbox_read(int32_t index, intptr_t ptr, int32_t cap);
ROOK_IMPORT(emit) int32_t rook_emit(int32_t channel_id, intptr_t ptr, int32_t len);
ROOK_IMPORT(set_timer) int32_t rook_set_timer(int64_t tick);
ROOK_IMPORT(rand) void rook_rand(intptr_t ptr, int32_t len);
ROOK_IMPORT(param) int32_t rook_param(intptr_t kptr, int32_t klen, intptr_t optr, int32_t ocap);
ROOK_IMPORT(log) void rook_log(int32_t level, intptr_t ptr, int32_t len);
ROOK_IMPORT(abort) [[noreturn]] void rook_abort(int32_t code, intptr_t ptr, int32_t len);
}

#ifdef __wasm__
// wasi-libc's libc++ objects import a handful of WASI functions even for
// code that never touches files or the environment. Defining them here keeps
// the module's import section at exactly the "rook" module. fd_write is what
// libc++ would use for stderr; it becomes a log line. proc_exit becomes abort.
extern "C" {
int32_t __imported_wasi_snapshot_preview1_fd_write(
  int32_t /*fd*/, int32_t iovs, int32_t iovs_len, int32_t nwritten)
{
  const auto* iov = reinterpret_cast<const int32_t*>(iovs);
  int32_t total = 0;
  for (int32_t i = 0; i < iovs_len; ++i)
  {
    const int32_t ptr = iov[2*i];
    const int32_t len = iov[2*i+1];
    rook_log(3, ptr, len);
    total += len;
  }
  *reinterpret_cast<int32_t*>(nwritten) = total;
  return 0;
}
int32_t __imported_wasi_snapshot_preview1_fd_close(int32_t) { return 8; /* EBADF */ }
int32_t __imported_wasi_snapshot_preview1_fd_prestat_get(int32_t, int32_t) { return 8; }
int32_t __imported_wasi_snapshot_preview1_fd_prestat_dir_name(int32_t, int32_t, int32_t) { return 8; }
int32_t __imported_wasi_snapshot_preview1_fd_seek(int32_t, int64_t, int32_t, int32_t) { return 8; }
int32_t __imported_wasi_snapshot_preview1_environ_get(int32_t, int32_t) { return 0; }
int32_t __imported_wasi_snapshot_preview1_environ_sizes_get(int32_t count, int32_t size)
{
  *reinterpret_cast<int32_t*>(count) = 0;
  *reinterpret_cast<int32_t*>(size) = 0;
  return 0;
}
[[noreturn]] void __imported_wasi_snapshot_preview1_proc_exit(int32_t code)
{
  rook_abort(100 + code, 0, 0);
}
}
#endif

namespace {

using rmf_traffic::blockade::Moderator;
using rmf_traffic::blockade::Writer;

// Channel ids and abort codes are part of the boundary map.
constexpr int32_t CH_SET = 10;
constexpr int32_t CH_READY = 11;
constexpr int32_t CH_REACHED = 12;
constexpr int32_t CH_RELEASE = 13;
constexpr int32_t CH_CANCEL = 14;
constexpr int32_t CH_HEARTBEAT_TIMER = 15;
constexpr int32_t CH_HEARTBEAT = 20;

constexpr int32_t ABORT_NO_PARAM = 2;
constexpr int32_t ABORT_UNKNOWN_CHANNEL = 3;
constexpr int32_t ABORT_MALFORMED = 4;
constexpr int32_t ABORT_EMIT_REFUSED = 5;
constexpr int32_t ABORT_INBOX_READ = 6;
constexpr int32_t ABORT_ID_TOO_WIDE = 7;
constexpr int32_t ABORT_PARTICIPANT_LIMIT = 8;

// One heartbeat is a 5-byte header plus one 49-byte status per participant.
// Keep it within the host ABI's 64 KiB per-call transfer limit. A new
// participant is refused at CH_SET, before it can leave oversized state that
// fails later during an otherwise valid timer heartbeat.
constexpr std::size_t MAX_HEARTBEAT_PARTICIPANTS = (64 * 1024 - 5) / 49;

constexpr int32_t LOG_DEBUG = 0;
constexpr int32_t LOG_INFO = 1;
constexpr int32_t LOG_ERROR = 2;

Moderator* moderator = nullptr;
std::size_t last_assignment_version = 0;

void log_str(int32_t level, const std::string& text)
{
  rook_log(level, reinterpret_cast<intptr_t>(text.data()),
    static_cast<int32_t>(text.size()));
}

[[noreturn]] void die(int32_t code, const char* why)
{
  rook_abort(code, reinterpret_cast<intptr_t>(why),
    static_cast<int32_t>(std::strlen(why)));
}

// Little-endian cursor over one inbound payload. Any short read is a
// malformed capture and aborts the cell; a typed ROS message cannot be short.
class Reader
{
public:
  explicit Reader(const std::vector<uint8_t>& bytes) : _b(bytes) {}

  uint8_t u8() { need(1); return _b[_i++]; }
  uint16_t u16() { uint16_t v; take(&v, 2); return v; }
  uint32_t u32() { uint32_t v; take(&v, 4); return v; }
  uint64_t u64() { uint64_t v; take(&v, 8); return v; }
  // Upstream keys some maps by std::size_t, which is 32 bits on wasm32, so a
  // participant or checkpoint id >= 2^32 would silently alias. Refuse it.
  uint64_t id() { const uint64_t v = u64(); if (v >> 32) die(ABORT_ID_TOO_WIDE, "id >= 2^32"); return v; }
  double f64() { double v; take(&v, 8); return v; }
  std::string str(std::size_t n)
  {
    need(n);
    std::string s(reinterpret_cast<const char*>(&_b[_i]), n);
    _i += n;
    return s;
  }
  std::size_t remaining() const { return _b.size() - _i; }
  void done() const { if (_i != _b.size()) die(ABORT_MALFORMED, "trailing bytes"); }

private:
  void need(std::size_t n) const
  {
    if (n > _b.size() - _i)
      die(ABORT_MALFORMED, "short payload");
  }
  void take(void* out, std::size_t n)
  {
    need(n);
    std::memcpy(out, &_b[_i], n);
    _i += n;
  }
  const std::vector<uint8_t>& _b;
  std::size_t _i = 0;
};

class ByteWriter
{
public:
  void u8(uint8_t v) { _b.push_back(v); }
  void u32(uint32_t v) { raw(&v, 4); }
  void u64(uint64_t v) { raw(&v, 8); }
  const std::vector<uint8_t>& bytes() const { return _b; }
private:
  void raw(const void* p, std::size_t n)
  {
    const auto* c = static_cast<const uint8_t*>(p);
    _b.insert(_b.end(), c, c + n);
  }
  std::vector<uint8_t> _b;
};

// Mirrors BlockadeNode::publish_status, with one boundary decision: the
// statuses array is sorted by participant id instead of hash-map order.
void publish_heartbeat()
{
  const auto& ranges = moderator->assignments().ranges();
  const auto& statuses = moderator->statuses();

  std::vector<uint64_t> ids;
  ids.reserve(statuses.size());
  for (const auto& s : statuses)
    ids.push_back(s.first);
  std::sort(ids.begin(), ids.end());

  ByteWriter w;
  w.u8(moderator->has_gridlock() ? 1 : 0);
  w.u32(static_cast<uint32_t>(ids.size()));
  for (const uint64_t id : ids)
  {
    const auto& status = statuses.at(id);
    const auto& range = ranges.at(id);
    w.u64(id);
    w.u64(status.reservation);
    w.u8(status.last_ready.has_value() ? 1 : 0);
    w.u64(status.last_ready.value_or(0));
    w.u64(status.last_reached);
    w.u64(range.begin);
    w.u64(range.end);
  }

  const auto& bytes = w.bytes();
  const int32_t rc = rook_emit(CH_HEARTBEAT,
      reinterpret_cast<intptr_t>(bytes.data()),
      static_cast<int32_t>(bytes.size()));
  if (rc != 0)
    die(ABORT_EMIT_REFUSED, "emit refused");
}

// Mirrors BlockadeNode::check_for_updates.
void check_for_updates()
{
  const std::size_t version = moderator->assignments().version();
  if (version == last_assignment_version)
    return;
  last_assignment_version = version;
  publish_heartbeat();
}

void handle(int32_t channel, const std::vector<uint8_t>& payload)
{
  Reader r(payload);
  // Every branch mirrors one BlockadeNode subscription callback, including
  // the catch-and-log that swallows Moderator exceptions.
  try
  {
    switch (channel)
    {
      case CH_SET:
      {
        const uint64_t participant = r.id();
        const uint64_t reservation = r.u64();
        const double radius = r.f64();
        const uint32_t n = r.u32();
        // x + y + can_hold + name_len is the smallest checkpoint encoding.
        // Bound the allocation by bytes already present before reserve() can
        // throw and be mistaken for an exception from the moderator itself.
        constexpr std::size_t MIN_CHECKPOINT_BYTES = 8 + 8 + 1 + 2;
        if (n > r.remaining() / MIN_CHECKPOINT_BYTES)
          die(ABORT_MALFORMED, "checkpoint count exceeds payload");
        std::vector<Writer::Checkpoint> path;
        path.reserve(n);
        for (uint32_t i = 0; i < n; ++i)
        {
          const double x = r.f64();
          const double y = r.f64();
          const bool can_hold = r.u8() != 0;
          const uint16_t name_len = r.u16();
          std::string map_name = r.str(name_len);
          path.push_back(Writer::Checkpoint{Eigen::Vector2d{x, y}, std::move(map_name), can_hold});
        }
        r.done();
        const auto& statuses = moderator->statuses();
        if (statuses.find(participant) == statuses.end()
            && statuses.size() >= MAX_HEARTBEAT_PARTICIPANTS)
          die(ABORT_PARTICIPANT_LIMIT, "participant limit reached");
        moderator->set(participant, reservation, Writer::Reservation{std::move(path), radius});
        break;
      }
      case CH_READY:
      case CH_REACHED:
      case CH_RELEASE:
      {
        const uint64_t participant = r.id();
        const uint64_t reservation = r.u64();
        const uint64_t checkpoint = r.id();
        r.done();
        if (channel == CH_READY)
          moderator->ready(participant, reservation, checkpoint);
        else if (channel == CH_REACHED)
          moderator->reached(participant, reservation, checkpoint);
        else
          moderator->release(participant, reservation, checkpoint);
        break;
      }
      case CH_CANCEL:
      {
        const uint64_t participant = r.id();
        const bool all = r.u8() != 0;
        const uint64_t reservation = r.u64();
        r.done();
        if (all)
          moderator->cancel(participant);
        else
          moderator->cancel(participant, reservation);
        break;
      }
      case CH_HEARTBEAT_TIMER:
        r.done();
        publish_heartbeat();
        return;
      default:
        die(ABORT_UNKNOWN_CHANNEL, "unknown channel");
    }
  }
  catch (const std::exception& e)
  {
    log_str(LOG_ERROR, std::string("Exception due to update: ") + e.what());
  }
  check_for_updates();
}

} // namespace

extern "C" {

#ifdef __wasm__
#define ROOK_EXPORT(name) __attribute__((export_name(#name)))
#else
#define ROOK_EXPORT(name)
#endif

ROOK_EXPORT(rook_init)
void rook_init(uint32_t /*actor_id*/)
{
  static const char key[] = "min_conflict_angle";
  double angle = 0.0;
  const int32_t got = rook_param(
    reinterpret_cast<intptr_t>(key), sizeof(key) - 1,
    reinterpret_cast<intptr_t>(&angle), sizeof(angle));
  if (got != sizeof(angle))
    die(ABORT_NO_PARAM, "min_conflict_angle param missing");

  moderator = new Moderator(
    [](std::string msg) { log_str(LOG_INFO, msg); },
    [](std::string msg) { log_str(LOG_DEBUG, msg); },
    angle);
  last_assignment_version = moderator->assignments().version();
}

ROOK_EXPORT(rook_step)
void rook_step()
{
  const int32_t count = rook_inbox_len();
  for (int32_t i = 0; i < count; ++i)
  {
    const int64_t meta = rook_inbox_meta(i);
    const int32_t channel = static_cast<int32_t>(static_cast<uint64_t>(meta) >> 32);
    const uint32_t len = static_cast<uint32_t>(meta);
    std::vector<uint8_t> payload(len);
    const int32_t got = rook_inbox_read(i,
        reinterpret_cast<intptr_t>(payload.data()), static_cast<int32_t>(len));
    if (got != static_cast<int32_t>(len))
      die(ABORT_INBOX_READ, "inbox_read short");
    handle(channel, payload);
  }
}

}
