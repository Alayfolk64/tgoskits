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
#include <sys/vfs.h>
#include <unistd.h>

#include "writeback_flags.h"

#define SYNC_STATFS_FLAG 16

static int verify_statfs_sync(const char *path, int synchronous) {
    struct statfs path_stat;
    struct statfs fd_stat;
    if (syscall(SYS_statfs, path, &path_stat) != 0) {
        perror("statfs synchronous mount");
        return 1;
    }
    int fd = syscall(SYS_openat, AT_FDCWD, path, O_RDONLY | O_DIRECTORY, 0);
    if (fd < 0) {
        perror("open synchronous mount directory");
        return 1;
    }
    if (syscall(SYS_fstatfs, fd, &fd_stat) != 0) {
        perror("fstatfs synchronous mount");
        close(fd);
        return 1;
    }
    if (close(fd) != 0) {
        perror("close mount directory");
        return 1;
    }
    if (!!(path_stat.f_flags & SYNC_STATFS_FLAG) != synchronous ||
        !!(fd_stat.f_flags & SYNC_STATFS_FLAG) != synchronous) {
        fprintf(stderr, "FAIL: %s statfs=%lx fstatfs=%lx expected sync=%d\n",
                path, (unsigned long)path_stat.f_flags,
                (unsigned long)fd_stat.f_flags, synchronous);
        return 1;
    }
    return 0;
}

static int has_option(const char *options, const char *wanted) {
    size_t wanted_len = strlen(wanted);
    while (*options) {
        size_t len = strcspn(options, ",");
        if (len == wanted_len && memcmp(options, wanted, len) == 0) {
            return 1;
        }
        options += len;
        if (*options == ',') {
            options++;
        }
    }
    return 0;
}

static int verify_mountinfo_policy(const char *path, int synchronous) {
    FILE *stream = fopen("/proc/self/mountinfo", "r");
    if (!stream) {
        perror("open mountinfo for writeback policy");
        return 1;
    }
    char *line = NULL;
    size_t capacity = 0;
    int result = 1;
    while (getline(&line, &capacity, stream) >= 0) {
        char mountpoint[PATH_MAX], mount_options[256];
        char type[64], source[256], super_options[256];
        if (sscanf(line, "%*s %*s %*s %*s %4095s %255s", mountpoint, mount_options) != 2 ||
            strcmp(mountpoint, path) != 0) {
            continue;
        }
        const char *separator = strstr(line, " - ");
        if (!separator || sscanf(separator + 3, "%63s %255s %255s", type, source, super_options) != 3) {
            fprintf(stderr, "FAIL: malformed mountinfo line: %s", line);
            break;
        }
        if (has_option(super_options, "sync") != synchronous ||
            !has_option(super_options, "dirsync") ||
            has_option(mount_options, "sync") || has_option(mount_options, "dirsync")) {
            fprintf(stderr, "FAIL: wrong shared writeback options: %s", line);
            break;
        }
        result = 0;
        break;
    }
    if (result != 0) {
        fprintf(stderr, "FAIL: no matching writeback policy for %s\n", path);
    }
    if (ferror(stream)) {
        perror("read mountinfo writeback policy");
        result = 1;
    }
    free(line);
    if (fclose(stream) != 0) {
        perror("close mountinfo writeback policy");
        result = 1;
    }
    return result;
}

static int verify_shared_policy(const char *source, const char *bound, int synchronous) {
    if (verify_statfs_sync(source, synchronous) != 0 ||
        verify_statfs_sync(bound, synchronous) != 0 ||
        verify_mountinfo_policy(source, synchronous) != 0 ||
        verify_mountinfo_policy(bound, synchronous) != 0) {
        return 1;
    }
    return 0;
}

int verify_writeback_mount_flags(void) {
    char base[] = "/tmp/remount-sync-XXXXXX";
    if (!mkdtemp(base)) {
        perror("create writeback mount test directory");
        return 1;
    }
    char source[PATH_MAX], bound[PATH_MAX];
    snprintf(source, sizeof(source), "%s/source", base);
    snprintf(bound, sizeof(bound), "%s/bound", base);
    int source_created = 0, bound_created = 0;
    int source_mounted = 0, bound_mounted = 0;
    int result = 1;
    if (mkdir(source, 0700) != 0) {
        perror("mkdir writeback source");
        goto cleanup;
    }
    source_created = 1;
    if (mkdir(bound, 0700) != 0) {
        perror("mkdir writeback bind target");
        goto cleanup;
    }
    bound_created = 1;
    if (syscall(SYS_mount, "tmpfs", source, "tmpfs", MS_SYNCHRONOUS | MS_DIRSYNC, NULL) != 0) {
        perror("mount synchronous dirsync tmpfs");
        goto cleanup;
    }
    source_mounted = 1;
    if (syscall(SYS_mount, source, bound, NULL, MS_BIND, NULL) != 0) {
        perror("bind synchronous filesystem");
        goto cleanup;
    }
    bound_mounted = 1;
    if (verify_shared_policy(source, bound, 1) != 0) {
        goto cleanup;
    }
    if (syscall(SYS_mount, NULL, bound, NULL, MS_REMOUNT | MS_BIND, NULL) != 0) {
        perror("bind remount must preserve superblock policy");
        goto cleanup;
    }
    if (verify_shared_policy(source, bound, 1) != 0) {
        goto cleanup;
    }
    if (syscall(SYS_mount, NULL, source, NULL, MS_REMOUNT, NULL) != 0) {
        perror("remount clears sync but preserves dirsync");
        goto cleanup;
    }
    if (verify_shared_policy(source, bound, 0) != 0) {
        goto cleanup;
    }
    if (syscall(SYS_mount, NULL, bound, NULL, MS_REMOUNT | MS_SYNCHRONOUS, NULL) != 0) {
        perror("ordinary remount through bind updates shared sync");
        goto cleanup;
    }
    result = verify_shared_policy(source, bound, 1);

cleanup:
    if (bound_mounted && syscall(SYS_umount2, bound, 0) != 0) {
        perror("unmount writeback bind");
        result = 1;
    }
    if (source_mounted && syscall(SYS_umount2, source, 0) != 0) {
        perror("unmount writeback source");
        result = 1;
    }
    if (bound_created && rmdir(bound) != 0) {
        perror("remove writeback bind directory");
        result = 1;
    }
    if (source_created && rmdir(source) != 0) {
        perror("remove writeback source directory");
        result = 1;
    }
    if (rmdir(base) != 0) {
        perror("remove writeback test directory");
        result = 1;
    }
    return result;
}
