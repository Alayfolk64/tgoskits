#!/usr/bin/env python3
"""Check actual MTTCG translation of an AArch64 RCsc store/load pair.

Usage: python3 check-qemu-rcsc.py [qemu-system-aarch64]
This diskless diagnostic does not run or modify StarryOS or its profile disk.
"""

from pathlib import Path
import os
import subprocess
import sys
import tempfile


def main():
    if len(sys.argv) > 2:
        raise ValueError("usage: check-qemu-rcsc.py [qemu-system-aarch64]")
    qemu = sys.argv[1] if len(sys.argv) == 2 else "qemu-system-aarch64"
    tests = Path(__file__).resolve().parent
    workspace = tests.parents[3]
    os.environ["TMPDIR"] = str(workspace / "tmp")
    run = Path(tempfile.mkdtemp(prefix="qemu-rcsc-", dir=workspace / "tmp"))
    obj = run / "probe.o"
    elf = run / "probe.elf"
    log = run / "translation.log"
    subprocess.run(["aarch64-linux-gnu-as", str(tests / "qemu-rcsc-order.S"),
                    "-o", str(obj)], check=True)
    subprocess.run(["aarch64-linux-gnu-ld", "-Ttext=0x40080000",
                    "-o", str(elf), str(obj)], check=True)
    subprocess.run([qemu, "-accel", "tcg,thread=multi", "-machine", "virt,gic-version=3",
                    "-cpu", "cortex-a53", "-smp", "8", "-m", "8192M", "-nographic",
                    "-monitor", "none", "-semihosting", "-kernel", str(elf),
                    "-d", "op,out_asm", "-D", str(log)], check=True)
    translation = log.read_text()
    print(translation)
    print(f"Translation evidence: {log}")
    # This is an executed-code-generation contract, not source-text matching:
    # the instruction pair must produce an explicit StoreLoad barrier between
    # its guest store and load. QEMU 10.2.1 prints TCG_BAR_SC | TCG_MO_ALL
    # as "seq:all"; this includes TCG_MO_ST_LD (bit 0x02 in tcg-mo.h).
    operations = translation.split("OP:", 1)[1].split("OUT:", 1)[0]
    store = operations.index("qemu_st_i64")
    load = operations.index("qemu_ld_i64", store)
    between = operations[store:load]
    barriers = [line.strip() for line in between.splitlines() if line.strip().startswith("mb ")]
    if "mb seq:all" not in barriers:
        raise AssertionError("RCsc StoreLoad barrier missing between guest STLR and LDAR")
    print(f"Observed intervening barriers: {barriers}")


if __name__ == "__main__":
    main()
