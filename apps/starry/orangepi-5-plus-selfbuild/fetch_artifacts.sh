#!/bin/bash
set -euo pipefail

app_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$app_dir/../../.." && pwd)"
out_dir="$repo_root/target/starry-orangepi5plus-selfbuild/artifacts"
remote_output=/opt/starry-orangepi5plus-selfbuild/rootfs/output
linux_user="${BOARD_LINUX_USER:-orangepi}"
ssh_port="${BOARD_SSH_PORT:-22}"
host=""
run_id=""

usage() {
    cat <<'USAGE'
Usage:
  apps/starry/orangepi-5-plus-selfbuild/fetch_artifacts.sh \
    --host BOARD_IP [--run-id RUN_ID]
USAGE
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --host) host="${2:-}"; shift 2 ;;
        --run-id) run_id="${2:-}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
    esac
done

[[ "$host" =~ ^[A-Za-z0-9._-]+$ ]] || { echo "invalid --host" >&2; exit 2; }
[[ "$linux_user" =~ ^[A-Za-z_][A-Za-z0-9_-]*$ ]] \
    || { echo "invalid BOARD_LINUX_USER" >&2; exit 2; }
if ! [[ "$ssh_port" =~ ^[0-9]+$ ]]; then
    echo "invalid BOARD_SSH_PORT" >&2
    exit 2
fi
if ((10#$ssh_port < 1 || 10#$ssh_port > 65535)); then
    echo "invalid BOARD_SSH_PORT" >&2
    exit 2
fi

remote="${linux_user}@${host}"
ssh_args=(-p "$ssh_port")
if [ -z "$run_id" ]; then
    run_id="$(ssh "${ssh_args[@]}" "$remote" sudo -n cat "$remote_output/latest-run")"
fi
case "$run_id" in
    ''|*[!A-Za-z0-9._-]*) echo "invalid run id: $run_id" >&2; exit 1 ;;
esac

artifact_dir="$out_dir/$run_id"
mkdir -p "$artifact_dir"
ssh "${ssh_args[@]}" "$remote" sudo -n sync
rsync -a --info=stats2 \
    -e "ssh -p $ssh_port" \
    --rsync-path="sudo -n rsync" \
    "$remote:$remote_output/runs/$run_id/" "$artifact_dir/"

(
    cd "$artifact_dir"
    sha256sum -c SHA256SUMS
)
[ -s "$artifact_dir/starryos.elf" ] || { echo "self-built ELF is missing" >&2; exit 1; }
[ -s "$artifact_dir/starryos.bin" ] || { echo "self-built binary is missing" >&2; exit 1; }
if command -v file >/dev/null 2>&1; then
    file "$artifact_dir/starryos.elf" | grep -q 'ELF 64-bit.*ARM aarch64' \
        || { echo "self-built ELF is not AArch64" >&2; exit 1; }
fi
if command -v readelf >/dev/null 2>&1; then
    readelf -h "$artifact_dir/starryos.elf" | grep -q 'Machine:.*AArch64' \
        || { echo "self-built ELF header is not AArch64" >&2; exit 1; }
fi

echo "selfbuilt_run_id=$run_id"
echo "selfbuilt_artifact_dir=$artifact_dir"
echo "===STARRY-ORANGEPI5PLUS-SELFBUILD-HOST-PASS run=${run_id}==="
