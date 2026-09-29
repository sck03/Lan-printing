"""Exercise a native host without issuer keys, activation or physical devices."""
import http.cookiejar
import json
import os
import pathlib
import re
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

exe = str(pathlib.Path(sys.argv[1]).resolve(strict=True))
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
base = f"http://127.0.0.1:{port}"
opener = urllib.request.build_opener(
    urllib.request.ProxyHandler({}),
    urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
)


def get(path):
    with opener.open(base + path, timeout=3) as response:
        return json.load(response)


with tempfile.TemporaryDirectory(prefix="lanprint-host-") as root:
    args = [exe, "--no-tray", "--background", "--port", str(port), "--data-dir", root]
    log_path = pathlib.Path(root) / "host.log"
    identities = []
    for attempt in range(2):
        with log_path.open("ab") as log:
            process = subprocess.Popen(args, stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 30
                while True:
                    if process.poll() is not None:
                        raise RuntimeError("Host exited during startup")
                    try:
                        session = get("/api/session")
                        break
                    except (urllib.error.URLError, TimeoutError):
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(0.1)
                assert session["platform"] in ("windows", "macos", "linux"), session
                assert session["driverProfiles"] == (os.name == "nt"), session
                state = get("/api/license")
                assert not state["registered"] and state["localAdmin"], state
                assert re.fullmatch(r"LP1-[0-9A-F]{32}", state["machineCode"]), state
                identities.append(state["machineCode"])
                try:
                    get("/api/devices")
                    raise AssertionError("Unregistered device access must be denied")
                except urllib.error.HTTPError as error:
                    assert error.code == 403
                    assert json.load(error)["code"] == "license_required"
                second = subprocess.run(args, capture_output=True, timeout=15)
                assert second.returncode == 0, second.stderr.decode(errors="replace")
                assert process.poll() is None, "Second instance stopped the first host"
            except Exception:
                print(log_path.read_text(errors="replace"), file=sys.stderr)
                raise
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=5)
                    raise AssertionError("Host did not stop within 10 seconds")
            if os.name != "nt":
                assert process.returncode == 0, "SIGTERM must shut down gracefully"
    assert identities[0] == identities[1], "Machine identity changed after restart"
    assert (pathlib.Path(root) / "config.json").is_file()
print("PASS: startup, machine identity, licensing gate, instance lock, restart and shutdown")
