#include <cstdint>
#include <cstdio>
#include <cstring>
#include <mutex>

#include <asm/prctl.h>
#include <fcntl.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#if !defined(__linux__) || !defined(__x86_64__)
#error "The MELE codec adapter requires Linux x86-64"
#endif

extern "C" {
void *LoadLibrary(const char *filename);
void *GetProcAddress(void *library, const char *name);
}

namespace {
constexpr std::uint64_t codec_size = 1007616;
constexpr std::uint64_t max_block = 64 * 1024 * 1024;
constexpr std::uint64_t max_compressed = max_block + max_block / 2;
constexpr unsigned char probe[] = {0x48, 0x83, 0xec, 0x10, 0x4c, 0x89, 0x14, 0x24,
                                    0x4c, 0x89, 0x5c, 0x24, 0x08, 0x4d, 0x33, 0xdb};

using Bound = std::int64_t (__attribute__((ms_abi)) *)(int, std::int64_t);
using Compress = std::int64_t (__attribute__((ms_abi)) *)(int, const void *, std::int64_t,
    void *, int, void *, void *, void *, void *, std::int64_t);
using Decompress = std::int64_t (__attribute__((ms_abi)) *)(const void *, std::int64_t,
    void *, std::int64_t, int, int, int, void *, std::int64_t, void *, void *, void *, std::int64_t, int);

std::mutex codec_mutex;
void *library = nullptr;
Bound bound = nullptr;
Compress compress = nullptr;
Decompress decompress = nullptr;
unsigned long codec_gs = 0;
bool load_attempted = false;

class GsScope {
public:
    GsScope() : captured_(syscall(SYS_arch_prctl, ARCH_GET_GS, &previous_) == 0) {}
    bool captured() const { return captured_; }
    bool enter() const {
        return captured_ && syscall(SYS_arch_prctl, ARCH_SET_GS, codec_gs) == 0;
    }
    ~GsScope() {
        // Returning to managed code with foreign thread state is unsafe.
        if (captured_ && syscall(SYS_arch_prctl, ARCH_SET_GS, previous_) != 0) {
            std::fputs("MELE codec failed to restore Linux thread state\n", stderr);
            _exit(126);
        }
    }
private:
    unsigned long previous_ = 0;
    bool captured_;
};

class Descriptor {
public:
    explicit Descriptor(int descriptor) : value(descriptor) {}
    ~Descriptor() { if (value >= 0) close(value); }
    const int value;
};
}

// Only the managed verifier may supply codec bytes; the loader patches this exact revision.
extern "C" __attribute__((visibility("default"))) int mele_oodle_load_verified(
    const unsigned char *bytes, std::uint64_t size) {
    std::lock_guard<std::mutex> lock(codec_mutex);
    if (!bytes || size != codec_size || bytes[0] != 'M' || bytes[1] != 'Z' ||
        std::memcmp(bytes + 0xbc8a0, probe, sizeof(probe)) != 0) return -1;
    if (load_attempted) return -2;
    load_attempted = true;
    GsScope gs;
    if (!gs.captured()) return -3;
    Descriptor memory(memfd_create("deployd-verified-oodle", MFD_CLOEXEC | MFD_ALLOW_SEALING));
    if (memory.value < 0) return -4;
    std::uint64_t written = 0;
    while (written < size) {
        const auto result = write(memory.value, bytes + written, size - written);
        if (result <= 0) return -4;
        written += static_cast<std::uint64_t>(result);
    }
    if (fcntl(memory.value, F_ADD_SEALS, F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE) != 0)
        return -4;
    char path[64];
    std::snprintf(path, sizeof(path), "/proc/self/fd/%d", memory.value);
    library = LoadLibrary(path);
    if (!library) return -5;
    if (syscall(SYS_arch_prctl, ARCH_GET_GS, &codec_gs) != 0) return -3;
    bound = reinterpret_cast<Bound>(GetProcAddress(library, "OodleLZ_GetCompressedBufferSizeNeeded"));
    compress = reinterpret_cast<Compress>(GetProcAddress(library, "OodleLZ_Compress"));
    decompress = reinterpret_cast<Decompress>(GetProcAddress(library, "OodleLZ_Decompress"));
    return bound && compress && decompress ? 0 : -6;
}

extern "C" __attribute__((visibility("default"))) std::int64_t mele_oodle_bound(std::uint64_t size) {
    std::lock_guard<std::mutex> lock(codec_mutex);
    if (!bound || size == 0 || size > max_block) return -1;
    GsScope gs;
    if (!gs.enter()) return -3;
    const auto result = bound(13, static_cast<std::int64_t>(size));
    return result > 0 && static_cast<std::uint64_t>(result) <= max_compressed ? result : -7;
}

extern "C" __attribute__((visibility("default"))) std::int64_t mele_oodle_compress(
    const unsigned char *source, std::uint64_t size, unsigned char *output, std::uint64_t capacity) {
    std::lock_guard<std::mutex> lock(codec_mutex);
    if (!source || !output || !bound || !compress || size == 0 || size > max_block || capacity > max_compressed)
        return -1;
    GsScope gs;
    if (!gs.enter()) return -3;
    const auto required = bound(13, static_cast<std::int64_t>(size));
    if (required <= 0 || static_cast<std::uint64_t>(required) > capacity) return -7;
    const auto result = compress(13, source, static_cast<std::int64_t>(size), output, 4,
        nullptr, nullptr, nullptr, nullptr, 0);
    return result > 0 && static_cast<std::uint64_t>(result) <= capacity ? result : -7;
}

extern "C" __attribute__((visibility("default"))) std::int64_t mele_oodle_decompress(
    const unsigned char *source, std::uint64_t size, unsigned char *output, std::uint64_t capacity) {
    std::lock_guard<std::mutex> lock(codec_mutex);
    if (!source || !output || !decompress || size < 2 || size > max_compressed || capacity == 0 || capacity > max_block)
        return -1;
    GsScope gs;
    if (!gs.enter()) return -3;
    const auto result = decompress(source, static_cast<std::int64_t>(size), output,
        static_cast<std::int64_t>(capacity), 1, 1, 0, nullptr, 0, nullptr, nullptr, nullptr, 0, 3);
    return result == static_cast<std::int64_t>(capacity) ? result : -7;
}
