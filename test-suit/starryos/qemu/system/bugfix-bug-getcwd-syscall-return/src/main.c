#define _GNU_SOURCE
#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

/* LTP getcwd01/02 cover invalid buffers and libc path results. These checks
 * cover the raw syscall return length and paths relative to a chroot. */
static int passed;
static int failed;

static void note_pass(const char *name)
{
    printf("PASS: %s\n", name);
    passed++;
}

static void note_fail(const char *name, const char *detail)
{
    printf("FAIL: %s: %s\n", name, detail);
    failed++;
}

static long getcwd_raw(char *buf, size_t size)
{
    return syscall(SYS_getcwd, buf, size);
}

static void expect_raw_getcwd_length(void)
{
    if (chdir("/tmp") != 0) {
        note_fail("chdir /tmp", strerror(errno));
        return;
    }

    char buf[128];
    memset(buf, 0xa5, sizeof(buf));
    errno = 0;
    long ret = getcwd_raw(buf, sizeof(buf));
    long expected = (long)strlen("/tmp") + 1;
    if (ret == expected && strcmp(buf, "/tmp") == 0) {
        note_pass("raw getcwd returns byte length including nul");
        return;
    }

    char detail[192];
    snprintf(detail, sizeof(detail),
             "ret=%ld errno=%d (%s) buf='%s', expected ret=%ld buf='/tmp'",
             ret, errno, strerror(errno), buf, expected);
    note_fail("raw getcwd length", detail);
}

static void expect_getcwd_relative_to_chroot(void)
{
    char root[128];
    snprintf(root, sizeof(root), "/tmp/starry-getcwd-chroot-%ld", (long)getpid());
    if (mkdir(root, 0700) != 0) {
        note_fail("mkdir chroot", strerror(errno));
        return;
    }

    char nested[160];
    snprintf(nested, sizeof(nested), "%s/nested", root);
    if (mkdir(nested, 0700) != 0) {
        note_fail("mkdir chroot nested", strerror(errno));
        return;
    }
    if (syscall(SYS_chroot, root) != 0) {
        note_fail("raw chroot", strerror(errno));
        return;
    }
    if (chdir("/nested") != 0) {
        note_fail("chdir inside chroot", strerror(errno));
        return;
    }

    char buf[128];
    errno = 0;
    long ret = getcwd_raw(buf, sizeof(buf));
    long expected = (long)strlen("/nested") + 1;
    if (ret == expected && strcmp(buf, "/nested") == 0) {
        note_pass("raw getcwd returns a path relative to the process root");
        return;
    }

    char detail[256];
    snprintf(detail, sizeof(detail),
             "ret=%ld errno=%d (%s) buf='%s', expected ret=%ld buf='/nested'",
             ret, errno, strerror(errno), buf, expected);
    note_fail("raw getcwd after chroot", detail);
}

int main(void)
{
    printf("=== bug-getcwd-syscall-return ===\n");

    expect_raw_getcwd_length();
    expect_getcwd_relative_to_chroot();

    printf("=== Results: %d passed, %d failed ===\n", passed, failed);
    if (failed == 0) {
        printf("ALL TESTS PASSED\n");
        return 0;
    }
    printf("SOME TESTS FAILED\n");
    return 1;
}
