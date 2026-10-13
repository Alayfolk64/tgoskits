#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>

static int expect_raw_cwd(const char *expected)
{
    char buf[PATH_MAX] = {0};
    long length = (long)strlen(expected) + 1;
    long ret = syscall(SYS_getcwd, buf, sizeof(buf));
    if (ret != length || strcmp(buf, expected) != 0) {
        fprintf(stderr, "FAIL: raw getcwd returned %ld '%s', expected %ld '%s'\n",
                ret, buf, length, expected);
        return 1;
    }
    errno = 0;
    if (syscall(SYS_getcwd, NULL, length - 1) != -1 || errno != ERANGE) {
        fprintf(stderr, "FAIL: raw getcwd must check size before pointer, errno=%d\n", errno);
        return 1;
    }
    errno = 0;
    if (syscall(SYS_getcwd, NULL, length) != -1 || errno != EFAULT) {
        fprintf(stderr, "FAIL: raw getcwd with sufficient size and NULL, errno=%d\n", errno);
        return 1;
    }
    return 0;
}

static int check_covered_root_cwd(const char *root, int outside)
{
    /* The saved process root is the covered location, not the new mount root.
     * Resolve relative to the outside cwd so lookup enters the new mount. */
    const char *relative_root = root + 1;
    if (mount("tmpfs", relative_root, "tmpfs", 0, NULL) != 0) {
        perror("FAIL: mount over process root");
        return 1;
    }
    int failed = 0;
    if (chdir(relative_root) != 0) {
        perror("FAIL: enter covering mount");
        failed = 1;
    } else if (expect_raw_cwd("/") != 0) {
        failed = 1;
    } else if (mkdir("nested", 0700) != 0 || chdir("nested") != 0) {
        perror("FAIL: enter covering mount descendant");
        failed = 1;
    } else {
        failed = expect_raw_cwd("/nested");
    }
    if (fchdir(outside) != 0 || umount(relative_root) != 0) {
        perror("FAIL: detach covering mount");
        failed = 1;
    }
    return failed;
}

static int check_chroot_cwd(const char *root)
{
    int outside = open("/", O_RDONLY | O_DIRECTORY);
    if (outside < 0 || syscall(SYS_chroot, root) != 0 || chdir("/") != 0) {
        perror("FAIL: prepare chroot cwd");
        return 1;
    }
    if (expect_raw_cwd("/") != 0) return 1;
    if (chdir("/nested") != 0) {
        perror("FAIL: chdir nested");
        return 1;
    }
    if (expect_raw_cwd("/nested") != 0) return 1;
    /* An open directory can remain outside the new process root. Test the raw
     * syscall because libc may reject Linux's non-absolute unreachable path. */
    if (fchdir(outside) != 0) {
        perror("FAIL: restore outside cwd");
        return 1;
    }
    if (expect_raw_cwd("(unreachable)/") != 0) return 1;
    int failed = check_covered_root_cwd(root, outside);
    if (close(outside) != 0) {
        perror("FAIL: close outside directory");
        return 1;
    }
    if (failed) return 1;
    printf("INFO: raw getcwd respects the process root and unreachable cwd\n");
    return 0;
}

static int test_chroot_cwd(void)
{
    char root[] = "/tmp/starry-root-cwd-XXXXXX";
    if (mkdtemp(root) == NULL) {
        perror("FAIL: mkdtemp");
        return 1;
    }
    char nested[sizeof(root) + sizeof("/nested")];
    snprintf(nested, sizeof(nested), "%s/nested", root);
    if (mkdir(nested, 0700) != 0) {
        perror("FAIL: mkdir nested");
        rmdir(root);
        return 1;
    }
    fflush(NULL);
    pid_t child = fork();
    if (child == 0) exit(check_chroot_cwd(root));
    int status = 0;
    int failed = child < 0 || waitpid(child, &status, 0) != child;
    if (failed) perror("FAIL: chroot child");
    failed = failed || !WIFEXITED(status) || WEXITSTATUS(status) != 0;
    if (rmdir(nested) != 0 || rmdir(root) != 0) {
        perror("FAIL: clean chroot directories");
        failed = 1;
    }
    return failed;
}

static int readlink_to(const char *path, char *buf, size_t bufsize)
{
    ssize_t n = readlink(path, buf, bufsize - 1);
    if (n < 0) {
        fprintf(stderr, "FAIL: readlink(%s): %s\n", path, strerror(errno));
        return -1;
    }
    buf[n] = '\0';
    return 0;
}

static int assert_symlink(const char *path)
{
    struct stat st;
    if (lstat(path, &st) != 0) {
        fprintf(stderr, "FAIL: lstat(%s): %s\n", path, strerror(errno));
        return -1;
    }
    if (!S_ISLNK(st.st_mode)) {
        fprintf(stderr, "FAIL: %s is not a symlink (mode=0%o)\n", path, st.st_mode);
        return -1;
    }
    return 0;
}

int main(void)
{
    char buf[PATH_MAX];
    char cwd[PATH_MAX];

    if (assert_symlink("/proc/self/root") < 0) return 1;
    printf("INFO: /proc/self/root is a symlink\n");

    if (readlink_to("/proc/self/root", buf, sizeof(buf)) < 0) return 1;
    if (strcmp(buf, "/") != 0) {
        fprintf(stderr, "FAIL: /proc/self/root -> '%s' (expected '/')\n", buf);
        return 1;
    }
    printf("INFO: /proc/self/root -> /\n");

    if (assert_symlink("/proc/self/cwd") < 0) return 1;
    printf("INFO: /proc/self/cwd is a symlink\n");

    if (getcwd(cwd, sizeof(cwd)) == NULL) {
        perror("FAIL: getcwd");
        return 1;
    }
    if (readlink_to("/proc/self/cwd", buf, sizeof(buf)) < 0) return 1;
    if (strcmp(buf, cwd) != 0) {
        fprintf(stderr, "FAIL: /proc/self/cwd -> '%s' (getcwd='%s')\n", buf, cwd);
        return 1;
    }
    printf("INFO: /proc/self/cwd -> %s (matches getcwd)\n", buf);

    if (chdir("/tmp") != 0) {
        fprintf(stderr, "FAIL: chdir(/tmp): %s\n", strerror(errno));
        return 1;
    }
    if (getcwd(cwd, sizeof(cwd)) == NULL) {
        perror("FAIL: getcwd after chdir");
        return 1;
    }
    if (readlink_to("/proc/self/cwd", buf, sizeof(buf)) < 0) return 1;
    if (strcmp(buf, "/tmp") != 0) {
        fprintf(stderr, "FAIL: after chdir, /proc/self/cwd -> '%s' (expected '/tmp')\n", buf);
        return 1;
    }
    printf("INFO: after chdir, /proc/self/cwd -> /tmp\n");

    if (readlink_to("/proc/self/root", buf, sizeof(buf)) < 0) return 1;
    if (strcmp(buf, "/") != 0) {
        fprintf(stderr, "FAIL: /proc/self/root changed after chdir: '%s'\n", buf);
        return 1;
    }

    if (chdir("/") != 0) {
        fprintf(stderr, "FAIL: chdir(/): %s\n", strerror(errno));
        return 1;
    }
    if (readlink_to("/proc/self/cwd", buf, sizeof(buf)) < 0) return 1;
    if (strcmp(buf, "/") != 0) {
        fprintf(stderr, "FAIL: after chdir(/), /proc/self/cwd -> '%s' (expected '/')\n", buf);
        return 1;
    }
    printf("INFO: after chdir(/), /proc/self/cwd -> /\n");

    if (test_chroot_cwd() != 0) return 1;
    printf("TEST_PROC_ROOT_CWD_PASSED\n");
    return 0;
}
