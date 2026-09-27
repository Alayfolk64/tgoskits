/*
 * bug-open-rdonly-append-promotes-rw: O_RDONLY|O_APPEND must NOT make the fd
 * writable. APPEND is a status flag and only affects WRITE behavior; it
 * doesn't grant write access.
 *
 * man 2 open §"O_APPEND":
 *   "The file is opened in append mode. Before each write(2), the file offset
 *    is positioned at the end of the file..."
 *   — APPEND is purely about write-time offset; combined with RDONLY it's
 *   effectively a no-op (no writes will happen).
 *
 * Linux behavior: open(file, O_RDONLY|O_APPEND); write(fd,...) → -1 EBADF
 * StarryOS bug: axfs-ng/highlevel/file.rs:to_flags() promotes
 *   (read=true, write=false, append=true) → READ|WRITE|APPEND, so the fd
 *   actually becomes writable.
 */
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

int main(void)
{
    const char *file = "/tmp/bug_rdonly_append";
    unlink(file);
    int fd0 = open(file, O_CREAT | O_WRONLY, 0644);
    if (fd0 < 0) { perror("setup"); return 1; }
    if (write(fd0, "hi", 2) != 2) { perror("setup write"); close(fd0); return 1; }
    if (close(fd0) != 0) { perror("setup close"); return 1; }

    int fd = open(file, O_RDONLY | O_APPEND);
    if (fd < 0) {
        printf("FAIL: open(file, O_RDONLY|O_APPEND) -> -1 errno=%d (%s); expected fd>=0\n",
               errno, strerror(errno));
        unlink(file);
        return 1;
    }

    /* read should succeed (RDONLY) */
    char buf[8] = {0};
    ssize_t r = read(fd, buf, sizeof(buf) - 1);
    int read_ok = (r == 2 && memcmp(buf, "hi", 2) == 0);

    /* write MUST fail with EBADF (RDONLY) */
    errno = 0;
    ssize_t w = write(fd, "X", 1);
    int write_errno = errno;
    int write_ok = (w == -1 && write_errno == EBADF);

    int ok = read_ok && write_ok;
    if (!ok) {
        printf("FAIL: read=%zd (want 2 + content 'hi') write=%zd errno=%d (%s) (want -1 EBADF)\n",
               r, w, write_errno, strerror(write_errno));
    }

    close(fd);

    /* A rejected append must not pre-seek to EOF, with or without sync flags.
     * These calls exercise the raw ABI and begin at a non-EOF cursor. */
    const int sync_flags[] = { 0, O_DSYNC, O_SYNC };
    for (size_t i = 0; i < sizeof(sync_flags) / sizeof(sync_flags[0]); ++i) {
        fd = syscall(SYS_openat, AT_FDCWD, file, O_RDONLY | O_APPEND | sync_flags[i], 0);
        if (fd < 0) { perror("open readonly append sync"); ok = 0; break; }
        errno = 0;
        long result = syscall(SYS_write, fd, "X", 1);
        int saved_errno = errno;
        long position = syscall(SYS_lseek, fd, 0, SEEK_CUR);
        if (result != -1 || saved_errno != EBADF || position != 0) {
            printf("FAIL: append flags=%#x write=%ld errno=%d (%s) cursor=%ld; "
                   "expected -1 EBADF and cursor=0\n", sync_flags[i], result,
                   saved_errno, strerror(saved_errno), position);
            ok = 0;
        }
        if (close(fd) != 0) { perror("close readonly append sync"); ok = 0; }
    }
    unlink(file);
    if (ok) {
        printf("PASS: readonly append rejects writes without moving the cursor, including sync flags\n");
    }
    return ok ? 0 : 1;
}
