#define _GNU_SOURCE

#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/timerfd.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

#define LINUX_TIME_UPTIME_SEC_MAX (30LL * 365 * 24 * 60 * 60)
#define LINUX_TIME_SETTOD_SEC_MAX                                           \
    (INT64_MAX / 1000000000LL - LINUX_TIME_UPTIME_SEC_MAX)

static const char *timestamp_path = "/tmp/starry-clock-settime-timestamp";

struct timerfd_probes {
    int relative_fd;
    int cancel_fd;
};

static int fail(const char *operation)
{
    fprintf(stderr, "FAIL: %s errno=%d (%s)\n", operation, errno,
            strerror(errno));
    puts("STARRY_GROUPED_TEST_FAILED: syscall-test-clock-settime");
    return EXIT_FAILURE;
}

static int64_t timespec_to_nanos(const struct timespec *time)
{
    return (int64_t)time->tv_sec * 1000000000LL + time->tv_nsec;
}

static struct timespec nanos_to_timespec(int64_t nanos)
{
    return (struct timespec){
        .tv_sec = nanos / 1000000000LL,
        .tv_nsec = nanos % 1000000000LL,
    };
}

static int expect_errno(long result, int expected, const char *operation)
{
    if (result == -1 && errno == expected) {
        return 0;
    }
    errno = EPROTO;
    return fail(operation);
}

static int check_unprivileged_set_is_rejected(const struct timespec *requested)
{
    pid_t child = fork();
    if (child < 0) {
        return fail("fork unprivileged clock setter");
    }
    if (child == 0) {
        if (setuid(65534) < 0) {
            _exit(2);
        }
        errno = 0;
        long result = syscall(SYS_clock_settime, CLOCK_REALTIME, requested);
        _exit(result == -1 && errno == EPERM ? 0 : 3);
    }

    int status = 0;
    if (waitpid(child, &status, 0) != child) {
        return fail("wait for unprivileged clock setter");
    }
    if (!WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        errno = EPROTO;
        return fail("reject unprivileged clock setter with EPERM");
    }
    return 0;
}

static int check_realtime_observers(int64_t requested_nanos,
                                    int64_t monotonic_before_nanos)
{
    struct timespec realtime = {0};
    struct timespec monotonic = {0};
    struct timeval wall = {0};

    if (clock_gettime(CLOCK_REALTIME, &realtime) < 0 ||
        clock_gettime(CLOCK_MONOTONIC, &monotonic) < 0 ||
        gettimeofday(&wall, NULL) < 0) {
        return fail("read clocks after clock_settime");
    }

    int64_t realtime_delta = timespec_to_nanos(&realtime) - requested_nanos;
    int64_t monotonic_delta =
        timespec_to_nanos(&monotonic) - monotonic_before_nanos;
    int64_t timeval_nanos =
        (int64_t)wall.tv_sec * 1000000000LL + wall.tv_usec * 1000LL;
    int64_t timeval_delta = timeval_nanos - requested_nanos;
    if (realtime_delta < 0 || realtime_delta > 5000000000LL ||
        monotonic_delta < 0 || monotonic_delta > 5000000000LL ||
        timeval_delta < 0 || timeval_delta > 5000000000LL) {
        errno = EPROTO;
        return fail("observe stepped realtime without changing monotonic time");
    }
    return 0;
}

static int check_new_file_timestamp(int64_t requested_nanos)
{
    unlink(timestamp_path);
    int fd = open(timestamp_path, O_CREAT | O_EXCL | O_WRONLY, 0600);
    if (fd < 0) {
        return fail("create timestamp probe");
    }
    if (write(fd, "x", 1) != 1) {
        close(fd);
        unlink(timestamp_path);
        return fail("write timestamp probe");
    }
    if (close(fd) < 0) {
        unlink(timestamp_path);
        return fail("close timestamp probe");
    }

    struct stat metadata = {0};
    int stat_result = stat(timestamp_path, &metadata);
    int saved_errno = errno;
    unlink(timestamp_path);
    errno = saved_errno;
    if (stat_result < 0) {
        return fail("stat timestamp probe");
    }

    int64_t timestamp_delta =
        timespec_to_nanos(&metadata.st_mtim) - requested_nanos;
    if (timestamp_delta < -1000000000LL ||
        timestamp_delta > 5000000000LL) {
        fprintf(stderr,
                "timestamp probe: requested=%lld mtime=%lld delta=%lld\n",
                (long long)requested_nanos,
                (long long)timespec_to_nanos(&metadata.st_mtim),
                (long long)timestamp_delta);
        errno = EPROTO;
        return fail("stamp new files from the adjusted realtime clock");
    }
    return 0;
}

static int prepare_timerfd_probes(const struct timespec *original_realtime,
                                  struct timerfd_probes *probes)
{
    probes->relative_fd = timerfd_create(CLOCK_MONOTONIC, TFD_NONBLOCK);
    probes->cancel_fd = timerfd_create(CLOCK_REALTIME, TFD_NONBLOCK);
    if (probes->relative_fd < 0 || probes->cancel_fd < 0) {
        return fail("create timerfd clock-step probes");
    }

    struct itimerspec relative = {
        .it_value = {.tv_sec = 30, .tv_nsec = 0},
    };
    struct itimerspec cancel = {
        .it_value = {
            .tv_sec = original_realtime->tv_sec + 600,
            .tv_nsec = original_realtime->tv_nsec,
        },
    };
    if (timerfd_settime(probes->relative_fd, 0, &relative, NULL) < 0 ||
        timerfd_settime(probes->cancel_fd,
                        TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET, &cancel,
                        NULL) < 0) {
        return fail("arm timerfd clock-step probes");
    }
    return 0;
}

static int check_timerfd_clock_step(const struct timerfd_probes *probes)
{
    uint64_t expirations = 0;
    usleep(100000);

    errno = 0;
    if (read(probes->relative_fd, &expirations, sizeof(expirations)) != -1 ||
        errno != EAGAIN) {
        errno = EPROTO;
        return fail("keep a relative timerfd on the monotonic clock");
    }

    errno = 0;
    if (read(probes->cancel_fd, &expirations, sizeof(expirations)) != -1 ||
        errno != ECANCELED) {
        errno = EPROTO;
        return fail("cancel an absolute realtime timerfd after clock step");
    }
    return 0;
}

static void close_timerfd_probes(const struct timerfd_probes *probes)
{
    if (probes->relative_fd >= 0) {
        close(probes->relative_fd);
    }
    if (probes->cancel_fd >= 0) {
        close(probes->cancel_fd);
    }
}

static int restore_realtime(const struct timespec *original_realtime,
                            const struct timespec *original_monotonic)
{
    struct timespec current_monotonic = {0};
    if (clock_gettime(CLOCK_MONOTONIC, &current_monotonic) < 0) {
        return fail("read monotonic time before restoring realtime");
    }
    int64_t elapsed = timespec_to_nanos(&current_monotonic) -
                      timespec_to_nanos(original_monotonic);
    struct timespec restored = nanos_to_timespec(
        timespec_to_nanos(original_realtime) + elapsed);
    if (syscall(SYS_clock_settime, CLOCK_REALTIME, &restored) < 0) {
        return fail("restore realtime clock");
    }
    return 0;
}

int main(void)
{
    struct timespec original_realtime = {0};
    struct timespec original_monotonic = {0};
    if (clock_gettime(CLOCK_REALTIME, &original_realtime) < 0 ||
        clock_gettime(CLOCK_MONOTONIC, &original_monotonic) < 0) {
        return fail("capture original clocks");
    }

    errno = 0;
    if (expect_errno(syscall(SYS_clock_settime, CLOCK_MONOTONIC,
                             (const void *)1),
                     EINVAL, "reject non-settable clock before pointer access")) {
        return EXIT_FAILURE;
    }

    errno = 0;
    if (expect_errno(syscall(SYS_clock_settime, CLOCK_REALTIME,
                             (const void *)1),
                     EFAULT, "reject an invalid realtime pointer")) {
        return EXIT_FAILURE;
    }

    struct timespec invalid = {.tv_sec = 1, .tv_nsec = 1000000000L};
    errno = 0;
    if (expect_errno(syscall(SYS_clock_settime, CLOCK_REALTIME, &invalid),
                     EINVAL, "reject invalid nanoseconds")) {
        return EXIT_FAILURE;
    }

    struct timespec before_monotonic = {.tv_sec = 0, .tv_nsec = 0};
    errno = 0;
    if (expect_errno(syscall(SYS_clock_settime, CLOCK_REALTIME,
                             &before_monotonic),
                     EINVAL, "reject realtime before the monotonic clock")) {
        return EXIT_FAILURE;
    }

    struct timespec outside_linux_ktime = {
        .tv_sec = LINUX_TIME_SETTOD_SEC_MAX,
        .tv_nsec = 0,
    };
    errno = 0;
    if (expect_errno(syscall(SYS_clock_settime, CLOCK_REALTIME,
                             &outside_linux_ktime),
                     EINVAL, "reject realtime outside Linux ktime range")) {
        return EXIT_FAILURE;
    }

    int64_t requested_nanos = timespec_to_nanos(&original_realtime) +
                              120LL * 1000000000LL;
    struct timespec requested = nanos_to_timespec(requested_nanos);
    if (check_unprivileged_set_is_rejected(&requested)) {
        return EXIT_FAILURE;
    }

    struct timerfd_probes probes = {.relative_fd = -1, .cancel_fd = -1};
    if (prepare_timerfd_probes(&original_realtime, &probes)) {
        close_timerfd_probes(&probes);
        return EXIT_FAILURE;
    }

    if (syscall(SYS_clock_settime, CLOCK_REALTIME, &requested) < 0) {
        close_timerfd_probes(&probes);
        return fail("set CLOCK_REALTIME as root");
    }

    int result = check_realtime_observers(
        requested_nanos, timespec_to_nanos(&original_monotonic));
    if (result == 0) {
        result = check_timerfd_clock_step(&probes);
    }
    if (result == 0) {
        result = check_new_file_timestamp(requested_nanos);
    }
    close_timerfd_probes(&probes);
    int restore_result = restore_realtime(&original_realtime,
                                          &original_monotonic);
    if (result != 0 || restore_result != 0) {
        return EXIT_FAILURE;
    }

    puts("STARRY_CLOCK_SETTIME_PASSED");
    puts("STARRY_GROUPED_TEST_PASSED: syscall-test-clock-settime");
    return EXIT_SUCCESS;
}
