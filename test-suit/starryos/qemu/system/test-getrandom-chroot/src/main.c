#define _GNU_SOURCE

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/random.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

#ifndef GRND_INSECURE
#define GRND_INSECURE 0x0004
#endif

static int fail(const char *operation)
{
    fprintf(stderr, "FAIL: %s: errno=%d\n", operation, errno);
    return 1;
}

static int probe_random_source(void)
{
    unsigned char bytes[1025];
    const unsigned int flags[] = {0, GRND_NONBLOCK, GRND_RANDOM, GRND_INSECURE};
    for (size_t i = 0; i < sizeof(flags) / sizeof(flags[0]); ++i) {
        if (syscall(SYS_getrandom, bytes, sizeof(bytes), flags[i]) != (long)sizeof(bytes)) {
            return fail("getrandom must fill a multi-chunk buffer without /dev");
        }
    }
    if (syscall(SYS_getrandom, NULL, 0, 0) != 0) {
        return fail("zero-length getrandom");
    }
    errno = 0;
    if (syscall(SYS_getrandom, (void *)(uintptr_t)1, 1, 0) != -1 || errno != EFAULT) {
        return fail("getrandom rejects an inaccessible destination");
    }
    errno = 0;
    if (syscall(SYS_getrandom, (void *)(uintptr_t)1, 1, 0x80000000U) != -1 || errno != EINVAL) {
        return fail("invalid getrandom flags precede destination access");
    }
    return 0;
}

int main(void)
{
    char root[] = "/run/starry-getrandom-XXXXXX";
    if (mkdtemp(root) == NULL) {
        return fail("create empty chroot");
    }
    pid_t child = fork();
    if (child < 0) {
        int result = fail("fork chroot probe");
        if (rmdir(root) != 0) {
            fail("remove unused chroot");
        }
        return result;
    }
    if (child == 0) {
        int result;
        if (syscall(SYS_chroot, root) != 0 || syscall(SYS_chdir, "/") != 0) {
            result = fail("enter empty chroot");
        } else {
            result = probe_random_source();
        }
        fflush(NULL);
        _exit(result);
    }
    int status = 0;
    pid_t waited;
    do {
        waited = waitpid(child, &status, 0);
    } while (waited == -1 && errno == EINTR);
    int result = 0;
    if (waited != child || !WIFEXITED(status) || WEXITSTATUS(status) != 0) {
        result = fail("random source must remain usable inside an empty chroot");
    }
    if (rmdir(root) != 0) {
        result = fail("remove empty chroot");
    }
    if (result == 0) {
        puts("STARRY_GROUPED_TEST_PASSED: test-getrandom-chroot");
    }
    return result;
}
