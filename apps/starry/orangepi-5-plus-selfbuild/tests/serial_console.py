"""Exercise pasted input through a real picocom process and two PTYs."""

import os
from pathlib import Path
import re
import select
import shutil
import subprocess
import time
import tty
import unittest


CONNECT_SERIAL = Path(__file__).resolve().parents[1] / "connect_serial.sh"


def read_until(descriptor, marker, timeout=5):
    received = bytearray()
    deadline = time.monotonic() + timeout
    while marker not in received and time.monotonic() < deadline:
        ready, _, _ = select.select([descriptor], [], [], 0.1)
        if ready:
            received.extend(os.read(descriptor, 65536))
    if marker not in received:
        raise AssertionError(f"did not receive {marker!r}: {bytes(received)!r}")
    return bytes(received)


@unittest.skipUnless(shutil.which("picocom"), "picocom is required for the PTY test")
class SerialConsoleTests(unittest.TestCase):
    def test_paste_does_not_send_terminal_wrapper_bytes_to_serial(self):
        terminal_master, terminal_slave = os.openpty()
        serial_master, serial_slave = os.openpty()
        process = None
        try:
            tty.setraw(serial_slave)
            process = subprocess.Popen(
                ["bash", str(CONNECT_SERIAL), os.ttyname(serial_slave)],
                stdin=terminal_slave,
                stdout=terminal_slave,
                stderr=terminal_slave,
                start_new_session=True,
            )
            output = read_until(terminal_master, b"Terminal ready")

            # Model a terminal whose previous application enabled bracketed
            # paste. Its renderer applies mode changes before encoding a paste.
            bracketed_paste = True
            for mode in re.findall(rb"\x1b\[\?2004([hl])", output):
                bracketed_paste = mode == b"h"
            pasted_command = b"echo SERIAL_PASTE_OK\r"
            keyboard_input = pasted_command
            if bracketed_paste:
                keyboard_input = b"\x1b[200~" + pasted_command + b"\x1b[201~"
            os.write(terminal_master, keyboard_input)
            received = read_until(serial_master, pasted_command)
            self.assertEqual(received, pasted_command)
        finally:
            if process is not None and process.poll() is None:
                os.write(terminal_master, b"\x01\x18")
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            for descriptor in (terminal_master, terminal_slave, serial_master, serial_slave):
                os.close(descriptor)


if __name__ == "__main__":
    unittest.main(verbosity=2)
