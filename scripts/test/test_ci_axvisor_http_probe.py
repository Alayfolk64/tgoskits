#!/usr/bin/env python3
"""Exercise the real HTTP probe frame readers with local socket streams."""

import importlib.util
import json
from pathlib import Path
import socket
import time
import unittest
from unittest.mock import Mock, patch


ROOT = Path(__file__).resolve().parents[2]
CASES = ROOT / "test-suit/axvisor/normal"


def load_probe(name, relative_path):
    spec = importlib.util.spec_from_file_location(name, CASES / relative_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


POOL = load_probe("pool_probe", "qemu-vm-pool-http/http_probe.py")
WEB = load_probe("web_probe", "qemu-web-ui/web-ui/http_probe.py")


def text_frame(value):
    payload = json.dumps(value).encode("utf-8")
    if len(payload) >= 126:
        raise ValueError("the fixture uses short WebSocket frames")
    return bytes([0x81, len(payload)]) + payload


def receive(probe, reader, websocket):
    # drain_available takes a duration; the other readers take a deadline.
    bound = 0.1 if reader == "drain_available" else time.monotonic() + 1.0
    return getattr(probe, reader)(websocket, bound)


class ProbeFrameTests(unittest.TestCase):
    def test_upgrade_and_first_events_can_share_one_read(self):
        snapshot = {"type": "snapshot", "vms": []}
        change = {"type": "created", "id": 2}
        frames = text_frame(snapshot) + text_frame(change)
        response = b"HTTP/1.1 101 Switching Protocols\r\n\r\n" + frames
        for probe, reader in ((POOL, "read_json_frame"), (WEB, "receive_text")):
            with self.subTest(reader=reader):
                client, server = socket.socketpair()
                with client, server:
                    server.sendall(response)
                    # Force one legal coalesced recv result, independently of
                    # how the OS splits a stream write into reads. The bytes
                    # still travel through the real socket; only chunking is
                    # controlled, not the production handshake/frame parser.
                    def receive_upgrade(length):
                        self.assertGreaterEqual(length, len(response))
                        data = b""
                        while len(data) < len(response):
                            chunk = client.recv(len(response) - len(data))
                            if not chunk:
                                raise AssertionError("fixture socket closed during upgrade")
                            data += chunk
                        return data

                    stream = Mock(wraps=client)
                    stream.recv.side_effect = receive_upgrade
                    with patch.object(probe.socket, "create_connection", return_value=stream):
                        upgraded = probe.open_websocket("/ws/events")
                    websocket = upgraded[0] if isinstance(upgraded, tuple) else upgraded
                    # This is a precondition for the coalesced-input regression;
                    # the decoded messages below are the observable verdict.
                    self.assertEqual(websocket.buffered, frames)
                    self.assertEqual(receive(probe, reader, websocket), snapshot)
                    self.assertEqual(receive(probe, reader, websocket), change)

    def test_event_and_console_readers_join_buffered_and_socket_bytes(self):
        message = {"type": "snapshot", "vms": []}
        frame = text_frame(message)
        payload = json.dumps(message).encode("utf-8")
        readers = (
            (POOL, "read_json_frame", message),
            (WEB, "receive_text", message),
            (WEB, "receive_some", payload),
            (WEB, "drain_available", payload),
        )
        for probe, reader, expected in readers:
            # Entirely buffered, split frame header, and entirely on the socket
            # cover the receive boundary without depending on TCP packet timing.
            for buffered_length in (len(frame), 1, 0):
                with self.subTest(reader=reader, buffered_length=buffered_length):
                    client, server = socket.socketpair()
                    with client, server:
                        client.settimeout(1.0)
                        websocket = probe.WebSocket(client, frame[:buffered_length])
                        server.sendall(frame[buffered_length:])
                        self.assertEqual(receive(probe, reader, websocket), expected)

    def test_partial_frames_cannot_extend_the_reader_deadline(self):
        frame = text_frame({"type": "snapshot", "vms": []})
        readers = ((POOL, "read_json_frame"), (WEB, "receive_text"),
                   (WEB, "receive_some"), (WEB, "drain_available"))
        for probe, reader in readers:
            for progress in (False, True):
                with self.subTest(reader=reader, progress=progress):
                    client, server = socket.socketpair()
                    with client, server:
                        client.settimeout(0.2)
                        stream = Mock(wraps=client)
                        websocket = probe.WebSocket(stream, frame[:1])
                        clock = [100.0]
                        if progress:
                            server.sendall(frame[1:])

                            def recv_fragment(length):
                                chunk = client.recv(min(length, 1))
                                clock[0] += 0.02
                                return chunk

                            stream.recv.side_effect = recv_fragment
                            clock_source = lambda: clock[0]
                            deadline = clock[0] + 0.05
                        else:
                            clock_source = time.monotonic
                            deadline = clock_source() + 0.01
                        duration = deadline - clock_source()
                        bound = duration if reader == "drain_available" else deadline
                        with patch.object(probe.time, "monotonic", side_effect=clock_source):
                            with self.assertRaisesRegex(TimeoutError, "frame.*deadline"):
                                getattr(probe, reader)(websocket, bound)
                        if not progress:
                            waits = [call.args[0] for call in stream.settimeout.call_args_list
                                     if call.args[0] != 0.2]
                            self.assertTrue(waits, "partial reads must use the remaining budget")
                            self.assertTrue(all(0 < wait <= duration for wait in waits))
                        # A failed partial read must not leave a shorter socket
                        # timeout installed for later cleanup or diagnostics.
                        self.assertEqual(client.gettimeout(), 0.2)

    def test_quiet_and_closed_event_streams_still_fail(self):
        for probe, reader in ((POOL, "read_json_frame"), (WEB, "receive_text")):
            with self.subTest(reader=reader, state="quiet"):
                client, server = socket.socketpair()
                with client, server:
                    websocket = probe.WebSocket(client, b"")
                    with self.assertRaises((TimeoutError, AssertionError)):
                        getattr(probe, reader)(websocket, time.monotonic() + 0.01)
            for buffered in (b"", b"\x81"):
                with self.subTest(reader=reader, state="closed", buffered=buffered):
                    client, server = socket.socketpair()
                    with client, server:
                        server.shutdown(socket.SHUT_WR)
                        websocket = probe.WebSocket(client, buffered)
                        with self.assertRaisesRegex(AssertionError, "closed"):
                            receive(probe, reader, websocket)


if __name__ == "__main__":
    unittest.main()
