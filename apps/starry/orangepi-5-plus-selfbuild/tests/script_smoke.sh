#!/bin/bash
set -euo pipefail

app_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

for script in \
    benchmark.sh \
    boot_script_is_starry.sh \
    boot_starry_once.sh \
    boot_starry_once_remote.sh \
    build_seed.sh \
    deploy_starry_boot_remote.sh \
    fetch_artifacts.sh \
    guest-selfbuild.sh \
    init.sh \
    install_source_link.sh \
    provision_rootfs.sh \
    provision_rootfs_remote.sh \
    restore_linux_boot.sh \
    run_linux_baseline.sh \
    run_linux_remote.sh \
    run_selfbuild.sh \
    set_guest_clock.sh \
    stage_starry_boot.sh \
    test_watchdog_reset.sh \
    validate_sha256.sh; do
    bash -n "$app_dir/$script"
done

for entrypoint in \
    boot_starry_once.sh \
    build_seed.sh \
    fetch_artifacts.sh \
    provision_rootfs.sh \
    run_linux_baseline.sh \
    run_selfbuild.sh \
    stage_starry_boot.sh \
    test_watchdog_reset.sh; do
    "$app_dir/$entrypoint" --help >/dev/null
done

python3 - "$app_dir/serial_selfbuild.py" <<'PY'
import ast
import pathlib
import sys

ast.parse(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
PY
python3 - "$app_dir/serial_watchdog_reset.py" <<'PY'
import ast
import pathlib
import sys

ast.parse(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8"))
PY

bash "$app_dir/tests/boot_script_identity.sh"
bash "$app_dir/tests/guest_clock.sh"

echo "orangepi5plus_selfbuild_script_smoke=PASS"
