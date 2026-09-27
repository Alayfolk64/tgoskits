#!/usr/bin/env bash
set -euo pipefail

app_dir="${STARRY_APP_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
workspace="${STARRY_WORKSPACE:-$(cd "$app_dir/../../.." && pwd)}"
overlay_dir="${STARRY_OVERLAY_DIR:-}"
rootfs="${STARRY_ROOTFS:-}"
rootfs_size_mib="${ROOTFS_SIZE_MIB:-16384}"
out_dir="$workspace/target/starry-macos-selfbuild"
profile_source_host="$out_dir/profile-source-host"
task_tmp_dir="$workspace/tmp"
selfbuild_mode="${STARRY_MACOS_SELFBUILD_MODE:-full}"
export COPYFILE_DISABLE=1
mkdir -p "$task_tmp_dir"
export TMPDIR="$task_tmp_dir"

usage() {
    cat <<'USAGE'
Usage:
  STARRY_ROOTFS=/path/to/rootfs.img STARRY_OVERLAY_DIR=/path/to/overlay \
    apps/starry/macos-selfbuild/prebuild.sh

Internal stage used by `cargo xtask starry app qemu`. It prepares the selected
app runner rootfs and assembles the overlay that the app runner injects:

  1. resizes the app runner rootfs with `cargo xtask image resize`;
  2. copies the prepared guest toolchain overlay cache;
  3. archives the current checkout as /opt/tgoskits-src.tar;
  4. copies Cargo registry cache archives needed for offline guest Cargo;
  5. writes source metadata and the guest runner under /opt.

This script does not inject the overlay and does not launch QEMU. The Starry app
runner owns both steps. Run full_self_build.sh for the default end-to-end flow.
USAGE
}

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
    usage
    exit 0
fi

if [[ "$#" -gt 0 ]]; then
    echo "unknown argument: $1" >&2
    usage >&2
    exit 2
fi

case "$selfbuild_mode" in
    full|tg-xtask-profile) ;;
    *) echo "unknown STARRY_MACOS_SELFBUILD_MODE: $selfbuild_mode" >&2; exit 2 ;;
esac

if [[ -z "$overlay_dir" ]]; then
    echo "error: STARRY_OVERLAY_DIR is required" >&2
    exit 1
fi

if [[ -z "$rootfs" ]]; then
    echo "error: STARRY_ROOTFS is required" >&2
    exit 1
fi

if [[ ! -f "$workspace/Cargo.toml" ]]; then
    echo "error: STARRY_WORKSPACE does not look like TGOSKits: $workspace" >&2
    exit 1
fi

tar_create_flags=()

detect_tar_create_flags() {
    local flag list
    list="$(mktemp -p "$task_tmp_dir" starry-tar-flags.XXXXXX)"
    : >"$list"
    for flag in --no-xattrs --no-fflags --no-mac-metadata --disable-copyfile; do
        if tar "$flag" -cf /dev/null -T "$list" >/dev/null 2>&1; then
            tar_create_flags+=("$flag")
        fi
    done
    rm -f "$list"
}

tar_create() {
    tar "${tar_create_flags[@]}" "$@"
}

git_value() {
    local fallback="$1"
    shift
    git -C "$workspace" "$@" 2>/dev/null || printf '%s\n' "$fallback"
}

detect_tar_create_flags

cargo_lock_registry_crates() {
    awk '
    function unquote(value) {
        sub(/^[^"]*"/, "", value)
        sub(/"$/, "", value)
        return value
    }

    function flush_package() {
        if (name != "" && version != "" && source == "registry+https://github.com/rust-lang/crates.io-index") {
            print name "-" version
        }
    }

    /^\[\[package\]\]/ {
        flush_package()
        name = ""
        version = ""
        source = ""
        next
    }

    /^name = / {
        name = unquote($0)
        next
    }

    /^version = / {
        version = unquote($0)
        next
    }

    /^source = / {
        source = unquote($0)
        next
    }

    END {
        flush_package()
    }
    ' "$workspace/Cargo.lock" | sort -u
}

copy_cargo_registry_cache() {
    local host_cargo_home cache_root overlay_cache_root missing_file crate
    local found candidate registry_dir candidate_root
    local -a cache_roots=()

    host_cargo_home="${CARGO_HOME:-$HOME/.cargo}"
    cache_root="${CARGO_REGISTRY_CACHE:-$host_cargo_home/registry/cache}"
    overlay_cache_root="$overlay_dir/root/.cargo/registry/cache"
    missing_file="$out_dir/missing-cargo-registry-cache.txt"
    cargo_registry_cache_count=0

    if [[ -d "$overlay_cache_root" ]]; then
        cache_roots+=("$overlay_cache_root")
    fi
    if [[ -d "$cache_root" ]]; then
        cache_roots+=("$cache_root")
    fi

    if [[ "${#cache_roots[@]}" = "0" ]]; then
        cat >&2 <<EOF
error: Cargo registry cache was not found in the toolchain overlay or at $cache_root.
Run cargo fetch --locked on the host, then retry.
EOF
        exit 1
    fi

    mkdir -p "$overlay_cache_root"
    : >"$missing_file"

    while IFS= read -r crate; do
        [[ -n "$crate" ]] || continue

        found=""
        for candidate_root in "${cache_roots[@]}"; do
            for candidate in "$candidate_root"/*/"${crate}.crate"; do
                if [[ -f "$candidate" ]]; then
                    found="$candidate"
                    break 2
                fi
            done
        done

        if [[ -z "$found" ]]; then
            printf '%s\n' "$crate" >>"$missing_file"
            continue
        fi

        registry_dir="$(basename "$(dirname "$found")")"
        mkdir -p "$overlay_cache_root/$registry_dir"
        if [[ "$found" != "$overlay_cache_root/$registry_dir/${crate}.crate" ]]; then
            cp "$found" "$overlay_cache_root/$registry_dir/"
        fi
        cargo_registry_cache_count=$((cargo_registry_cache_count + 1))
    done < <(cargo_lock_registry_crates)

    if [[ -s "$missing_file" ]]; then
        cat >&2 <<EOF
error: host Cargo registry cache is missing crates required by Cargo.lock:
$(sed 's/^/  /' "$missing_file")

Fetch them on the host, then rebuild the rootfs:
  cargo fetch --locked
EOF
        exit 1
    fi

    rm -f "$missing_file"
}

copy_overlay_tree() {
    local src="$1"
    local dst="$2"

    if [[ ! -d "$src" ]]; then
        echo "toolchain overlay cache not found: $src" >&2
        echo "run apps/starry/macos-selfbuild/prepare_toolchain_overlay.sh first, or rerun the full flow" >&2
        exit 1
    fi

    (cd "$src" && tar_create -cf - .) | (cd "$dst" && tar xf -)
}

resize_rootfs() {
    if [[ "${RESIZE_ROOTFS:-1}" != "1" ]]; then
        printf '%s\n' "$rootfs" >"$out_dir/rootfs.path"
        return 0
    fi

    if [[ ! -f "$rootfs" ]]; then
        echo "rootfs image not found: $rootfs" >&2
        exit 1
    fi

    (cd "$workspace" && cargo xtask image resize "$rootfs" --size-mib "$rootfs_size_mib")
    printf '%s\n' "$rootfs" >"$out_dir/rootfs.path"
    echo "rootfs=$rootfs"
    echo "rootfs_size_mib=$rootfs_size_mib"
}

prepare_toolchain_overlay() {
    local toolchain_overlay_dir
    toolchain_overlay_dir="${STARRY_TOOLCHAIN_OVERLAY_DIR:-$out_dir/rootfs-build/toolchain-overlay}"
    "$app_dir/prepare_toolchain_overlay.sh" --output "$toolchain_overlay_dir"

    copy_overlay_tree "$toolchain_overlay_dir" "$overlay_dir"
    echo "toolchain_overlay=$toolchain_overlay_dir"
}

profile_tg_xtask_source_fingerprint() {
    (
        cd "$profile_source_host"
        {
            local path tree
            for path in Cargo.toml Cargo.lock rust-toolchain.toml; do
                [[ ! -f "$path" ]] || sha256sum "$path"
            done
            for tree in xtask scripts/axbuild; do
                find "$tree" -type f -print | LC_ALL=C sort | while IFS= read -r path; do
                    sha256sum "$path"
                done
            done
        } | sha256sum | sed 's/[[:space:]].*$//'
    )
}

prepare_profile_source_host_cache() {
    [[ "$selfbuild_mode" == "tg-xtask-profile" ]] || return 0

    local archive_hash source_stamp
    archive_hash="$(sha256sum "$src_tar" | sed 's/[[:space:]].*$//')"
    source_stamp="$profile_source_host/.source.tar.sha256"
    if [[ -f "$profile_source_host/Cargo.toml" \
        && -f "$source_stamp" \
        && "$(sed -n '1p' "$source_stamp")" == "$archive_hash" ]]; then
        echo "profile_source_host_reused=true"
        return 0
    fi

    rm -rf "$profile_source_host"
    mkdir -p "$profile_source_host"
    tar -xf "$src_tar" -C "$profile_source_host"
    printf '%s\n' "$archive_hash" >"$source_stamp"
    echo "profile_source_host_reused=false"
    echo "profile_source_host_sha256=$archive_hash"
}

prepare_profile_tg_xtask() {
    [[ "$selfbuild_mode" == "tg-xtask-profile" ]] || return 0

    # Cross-build the orchestration tool once so the measured guest run starts
    # directly at the ArceOS build instead of bootstrapping tg-xtask in StarryOS.
    # shellcheck source=prepare_host_tools.sh
    source "$app_dir/prepare_host_tools.sh"
    prepare_macos_selfbuild_host_tools

    local profile_host_target profile_tg_xtask_bin profile_tg_xtask_stamp
    local source_fingerprint cc ar
    profile_host_target="${STARRY_PROFILE_HOST_TARGET_DIR:-$workspace/target/starry-profile-host-tools}"
    profile_tg_xtask_bin="$profile_host_target/aarch64-unknown-linux-musl/release/tg-xtask"
    profile_tg_xtask_stamp="$profile_host_target/tg-xtask.source.sha256"
    source_fingerprint="$(profile_tg_xtask_source_fingerprint)"
    cc="$(command -v aarch64-linux-musl-gcc || command -v aarch64-linux-musl-cc)"
    ar="$(command -v aarch64-linux-musl-ar)"

    if [[ ! -x "$profile_tg_xtask_bin" \
        || ! -f "$profile_tg_xtask_stamp" \
        || "$(sed -n '1p' "$profile_tg_xtask_stamp")" != "$source_fingerprint" ]]; then
        (
            cd "$profile_source_host"
            CARGO_TARGET_DIR="$profile_host_target" \
                CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="$cc" \
                CC_aarch64_unknown_linux_musl="$cc" \
                AR_aarch64_unknown_linux_musl="$ar" \
                cargo build --locked --release -p tg-xtask --target aarch64-unknown-linux-musl
        )
        printf '%s\n' "$source_fingerprint" >"$profile_tg_xtask_stamp"
    fi

    mkdir -p "$overlay_dir/opt/tgoskits-profile/bin"
    install -m 0755 "$profile_tg_xtask_bin" \
        "$overlay_dir/opt/tgoskits-profile/bin/tg-xtask"
    install -m 0644 "$profile_tg_xtask_stamp" \
        "$overlay_dir/opt/tgoskits-profile/tg-xtask.source.sha256"
    echo "profile_tg_xtask=$profile_tg_xtask_bin"
    echo "profile_tg_xtask_source_fingerprint=$source_fingerprint"
}

prepare_profile_source_overlay() {
    [[ "$selfbuild_mode" == "tg-xtask-profile" ]] || return 0

    local archive_hash rootfs_hash profile_source_overlay
    archive_hash="$(sha256sum "$src_tar" | sed 's/[[:space:]].*$//')"
    rootfs_hash=""
    if [[ "${STARRY_PROFILE_REFRESH_ROOTFS_SOURCE:-0}" != "1" ]] \
        && command -v debugfs >/dev/null 2>&1; then
        rootfs_hash="$(
            debugfs -R 'cat /opt/tgoskits-profile/source.tar.sha256' "$rootfs" 2>/dev/null \
                | tr -d '\r' \
                | sed -n '1p'
        )"
    fi

    if [[ "$rootfs_hash" == "$archive_hash" ]]; then
        echo "profile_rootfs_source_reused=true"
        return 0
    fi

    profile_source_overlay="$overlay_dir/opt/tgoskits-profile/source"
    rm -rf "$profile_source_overlay"
    mkdir -p "$profile_source_overlay"
    tar -xf "$src_tar" -C "$profile_source_overlay"
    printf '%s\n' "$archive_hash" \
        >"$overlay_dir/opt/tgoskits-profile/source.tar.sha256"
    echo "profile_rootfs_source_reused=false"
    echo "profile_rootfs_source_sha256=$archive_hash"
}

actual_commit="$(git_value unknown rev-parse HEAD)"
if [[ -n "${TGOSKITS_COMMIT:-}" && "$actual_commit" != "unknown" && "$TGOSKITS_COMMIT" != "$actual_commit" ]]; then
    echo "error: TGOSKITS_COMMIT=$TGOSKITS_COMMIT does not match workspace HEAD $actual_commit" >&2
    exit 1
fi

source_commit="${TGOSKITS_COMMIT:-$actual_commit}"
source_ref="${TGOSKITS_REF:-$(git_value detached symbolic-ref --quiet --short HEAD)}"
dirty="unknown"
if git -C "$workspace" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    if [[ -n "$(git -C "$workspace" status --porcelain --untracked-files=all)" ]]; then
        dirty="true"
    else
        dirty="false"
    fi
fi

mkdir -p "$out_dir" "$overlay_dir/opt"
resize_rootfs
prepare_toolchain_overlay

meta_file="$out_dir/tgoskits-src.meta"
cat >"$meta_file" <<EOF
commit=$source_commit
ref=$source_ref
dirty=$dirty
generated_by=apps/starry/macos-selfbuild/prebuild.sh
EOF

meta_in_tar="$out_dir/.tgoskits-source-meta"
cp "$meta_file" "$meta_in_tar"

src_tar="$out_dir/tgoskits-src.tar"
profile_src_tar="$out_dir/arceos-helloworld-profile-source.tar"
profile_meta_file="$out_dir/arceos-helloworld-profile-source.meta"
profile_source_reused=false
if [[ "$selfbuild_mode" == "tg-xtask-profile" \
    && "${STARRY_PROFILE_REFRESH_SOURCE:-0}" != "1" \
    && -f "$profile_src_tar" \
    && -f "$profile_meta_file" ]]; then
    cp "$profile_src_tar" "$src_tar"
    cp "$profile_meta_file" "$meta_file"
    profile_source_reused=true
else
    tar_create -C "$workspace" \
        --exclude .git \
        --exclude target \
        --exclude tmp \
        --exclude .cache \
        --exclude .idea \
        --exclude .vscode \
        --exclude docs/node_modules \
        --exclude docs/build \
        -cf "$src_tar" .

    tar_create -C "$out_dir" -rf "$src_tar" .tgoskits-source-meta
    if [[ "$selfbuild_mode" == "tg-xtask-profile" ]]; then
        cp "$src_tar" "$profile_src_tar"
        cp "$meta_file" "$profile_meta_file"
    fi
fi

prepare_profile_source_host_cache
prepare_profile_source_overlay
prepare_profile_tg_xtask

cargo_registry_cache_count=0
copy_cargo_registry_cache

case "$selfbuild_mode" in
    full) guest_runner="$app_dir/guest-selfbuild.sh" ;;
    tg-xtask-profile) guest_runner="$app_dir/guest-tg-xtask-profile.sh" ;;
esac
install -m 0755 "$guest_runner" "$overlay_dir/opt/starry-macos-run.sh"
install -m 0644 "$src_tar" "$overlay_dir/opt/tgoskits-src.tar"
install -m 0644 "$meta_file" "$overlay_dir/opt/tgoskits-src.meta"

echo "macos-selfbuild overlay ready in $overlay_dir"
echo "selfbuild_mode=$selfbuild_mode"
echo "source_commit=$source_commit"
echo "source_ref=$source_ref"
echo "source_dirty=$dirty"
echo "profile_source_reused=$profile_source_reused"
echo "cargo_registry_cache_archives=$cargo_registry_cache_count"
