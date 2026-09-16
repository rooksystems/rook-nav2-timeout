// Native driver for cell.cpp. It compiles the same shim and vendored moderator
// with the system C++ toolchain, then feeds it the recording dumped by
// `rook-host rmf <cell> --dump <dir>`. It writes heartbeats in the host framing
// so `cmp` can compare native and Wasm decisions byte for byte.
//
// This is outside the replay-integrity guarantee. It measures the difference
// between native and Wasm builds.

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

extern "C" {
void rook_init(uint32_t actor_id);
void rook_step();
}

namespace {

struct Message {
  uint64_t tick;
  uint32_t channel;
  std::vector<uint8_t> payload;
};

std::vector<Message> g_inbox;
uint64_t g_tick = 0;
double g_min_conflict_angle = 0.0;
FILE* g_out = nullptr;

template<typename T>
bool read_raw(FILE* f, T* out) { return std::fread(out, sizeof(T), 1, f) == 1; }

} // namespace

extern "C" {
int64_t rook_tick() { return static_cast<int64_t>(g_tick); }
int32_t rook_inbox_len() { return static_cast<int32_t>(g_inbox.size()); }
int64_t rook_inbox_meta(int32_t index)
{
  const auto& m = g_inbox.at(static_cast<size_t>(index));
  return static_cast<int64_t>((static_cast<uint64_t>(m.channel) << 32) | m.payload.size());
}
int32_t rook_inbox_read(int32_t index, intptr_t ptr, int32_t cap)
{
  const auto& m = g_inbox.at(static_cast<size_t>(index));
  if (cap < static_cast<int32_t>(m.payload.size()))
    return -static_cast<int32_t>(m.payload.size());
  std::memcpy(reinterpret_cast<void*>(ptr), m.payload.data(), m.payload.size());
  return static_cast<int32_t>(m.payload.size());
}
int32_t rook_emit(int32_t channel_id, intptr_t ptr, int32_t len)
{
  if (channel_id != 20)
    return -1;
  const uint32_t len32 = static_cast<uint32_t>(len);
  std::fwrite(&g_tick, sizeof(g_tick), 1, g_out);
  std::fwrite(&len32, sizeof(len32), 1, g_out);
  std::fwrite(reinterpret_cast<const void*>(ptr), 1, len32, g_out);
  return 0;
}
int32_t rook_set_timer(int64_t) { return 0; }
void rook_rand(intptr_t, int32_t) { std::abort(); }
int32_t rook_param(intptr_t kptr, int32_t klen, intptr_t optr, int32_t ocap)
{
  const std::string key(reinterpret_cast<const char*>(kptr), static_cast<size_t>(klen));
  if (key != "min_conflict_angle")
    return -1;
  if (ocap < static_cast<int32_t>(sizeof(double)))
    return -static_cast<int32_t>(sizeof(double));
  std::memcpy(reinterpret_cast<void*>(optr), &g_min_conflict_angle, sizeof(double));
  return sizeof(double);
}
void rook_log(int32_t, intptr_t, int32_t) {}
[[noreturn]] void rook_abort(int32_t code, intptr_t ptr, int32_t len)
{
  std::fprintf(stderr, "cell aborted with code %d: %.*s\n", code, len,
    reinterpret_cast<const char*>(ptr));
  std::exit(70);
}
}

int main(int argc, char** argv)
{
  if (argc != 4)
  {
    std::fprintf(stderr, "usage: driver <deliveries.bin> <heartbeats.out> <min_conflict_angle_radians>\n");
    return 2;
  }
  FILE* in = std::fopen(argv[1], "rb");
  g_out = std::fopen(argv[2], "wb");
  if (!in || !g_out)
    return 2;
  g_min_conflict_angle = std::strtod(argv[3], nullptr);

  std::vector<Message> all;
  for (;;)
  {
    Message m;
    uint32_t len = 0;
    if (!read_raw(in, &m.tick)) break;
    if (!read_raw(in, &m.channel) || !read_raw(in, &len)) return 3;
    m.payload.resize(len);
    if (len && std::fread(m.payload.data(), 1, len, in) != len) return 3;
    all.push_back(std::move(m));
  }
  std::fclose(in);

  rook_init(1);
  size_t i = 0;
  while (i < all.size())
  {
    g_tick = all[i].tick;
    g_inbox.clear();
    while (i < all.size() && all[i].tick == g_tick)
      g_inbox.push_back(all[i++]);
    rook_step();
  }
  std::fclose(g_out);
  return 0;
}
