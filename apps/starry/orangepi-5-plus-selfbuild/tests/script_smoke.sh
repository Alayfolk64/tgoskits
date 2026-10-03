#!/bin/bash
set -euo pipefail

app_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for script in \
    arm_selfbuild_watchdog.sh \
    benchmark.sh \
    boot_script_is_starry.sh \
    boot_starry_once.sh \
    boot_starry_once_remote.sh \
    connect_serial.sh \
    configure_linux_watchdog.sh \
    deploy_starry_boot_remote.sh \
    ensure_linux_fsck.sh \
    fetch_artifacts.sh \
    guest-selfbuild.sh \
    guest-tg-xtask-bench.sh \
    init.sh \
    init-kernel-selfbuild.sh \
    init-tg-xtask-bench.sh \
    init-watchdog-smoke.sh \
    install_source_link.sh \
    linux-run-tg-xtask-bench.sh \
    provision_rootfs.sh \
    provision_rootfs_remote.sh \
    restore_linux_boot.sh \
    run_linux_baseline.sh \
    run_linux_remote.sh \
    run_selfbuild.sh \
    set_guest_clock.sh \
    stage_starry_boot.sh \
    validate_sha256.sh; do
    bash -n "$app_dir/$script"
done

for entrypoint in \
    boot_starry_once.sh \
    connect_serial.sh \
    fetch_artifacts.sh \
    provision_rootfs.sh \
    run_linux_baseline.sh \
    run_selfbuild.sh \
    stage_starry_boot.sh; do
    "$app_dir/$entrypoint" --help >/dev/null
done

python3 - "$app_dir/serial_selfbuild.py" "$app_dir/patch-secret-service-derive.py" <<'PY'
import ast
import pathlib
import sys

for source in sys.argv[1:]:
    ast.parse(pathlib.Path(source).read_text(encoding="utf-8"))
PY

bash "$app_dir/tests/boot_script_identity.sh"
bash "$app_dir/tests/guest_clock.sh"
bash "$app_dir/tests/install_source_link.sh"
bash "$app_dir/tests/linux_recovery.sh"
bash "$app_dir/tests/provision_rootfs.sh"
bash "$app_dir/tests/selfbuild_contract.sh"
bash "$app_dir/tests/validate_sha256.sh"
python3 "$app_dir/tests/serial_console.py"

echo "orangepi5plus_selfbuild_script_smoke=PASS"
