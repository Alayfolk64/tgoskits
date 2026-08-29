#!/usr/bin/env python3
"""Restore Linux boot selection and capture a deliberate watchdog reset."""

from __future__ import annotations

import argparse
import os
import pathlib
import sys
import time

import serial


SHELL_PROMPT = b"root@starry:/root #"
RESTORE_COMMAND = b"sh /opt/starry-orangepi5plus-selfbuild/restore_linux_boot.sh\r"
ARMED = b"self-build watchdog reset test armed"
RESTORED = b"===STARRY-ORANGEPI5PLUS-SELFBUILD-LINUX-BOOT-RESTORED"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--serial", required=True, dest="serial_device")
    parser.add_argument("--log", required=True, type=pathlib.Path)
    parser.add_argument("--ready-file", required=True, type=pathlib.Path)
    parser.add_argument("--timeout", type=int, default=300)
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    return args


def main() -> int:
    args = parse_args()
    args.log.parent.mkdir(parents=True, exist_ok=True)
    args.ready_file.parent.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + args.timeout
    window = b""
    command_sent = False

    with args.log.open("ab", buffering=0) as log_file:
        with serial.Serial(
            args.serial_device,
            baudrate=1_500_000,
            timeout=0.25,
            write_timeout=2,
            exclusive=True,
        ) as uart:
            args.ready_file.write_text(
                f"pid={os.getpid()}\nserial={args.serial_device}\n",
                encoding="utf-8",
            )
            print(f"serial_watchdog_ready={args.serial_device}", flush=True)
            while time.monotonic() < deadline:
                data = uart.read(uart.in_waiting or 1)
                if not data:
                    continue
                log_file.write(data)
                log_file.flush()
                sys.stdout.buffer.write(data)
                sys.stdout.buffer.flush()
                window = (window + data)[-131_072:]

                if not command_sent and SHELL_PROMPT in window:
                    uart.write(RESTORE_COMMAND)
                    uart.flush()
                    command_sent = True
                    print("serial_watchdog_restore_sent=1", flush=True)
                if ARMED in window and RESTORED in window:
                    print("serial_watchdog_armed_and_linux_restored=1", flush=True)
                    return 0

    print("serial_watchdog_timeout=1", file=sys.stderr)
    return 124


if __name__ == "__main__":
    raise SystemExit(main())
