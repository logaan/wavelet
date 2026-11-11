#!/usr/bin/env python3
import socket
import subprocess
import time
import urllib.request
from pathlib import Path

root = Path(__file__).resolve().parents[1]
with socket.socket() as probe:
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
server = subprocess.Popen(
    ["wasmtime", "serve", "--addr", f"127.0.0.1:{port}", str(root / "out/app.wasm")],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.PIPE,
)
try:
    for attempt in range(80):
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/hello?a=1&b=2", timeout=1) as response:
                body = response.read().decode()
                assert response.status == 200
                assert response.headers["Content-Type"] == "text/html; charset=utf-8"
                assert "Hello, world!" in body
                assert "/hello?a=1&amp;b=2" in body
                print("PASS HTTP example: status, content type, greeting, escaped query")
                break
        except OSError:
            if server.poll() is not None:
                raise RuntimeError(server.stderr.read().decode())
            time.sleep(0.1)
    else:
        raise RuntimeError("HTTP server did not start")
finally:
    server.terminate()
    server.wait()
