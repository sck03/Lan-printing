"""Fail CI if private issuer tools, keys or generated licenses are tracked."""
import pathlib
import subprocess
import sys

paths = subprocess.check_output(["git", "ls-files", "-z"]).decode().split("\0")
blocked = []
for path in filter(None, paths):
    lower = path.lower()
    name = pathlib.PurePosixPath(lower).name
    if (
        lower.startswith(("admin-tools/", "target/", "dist/", "release/", "artifacts/"))
        or "license-generator" in name
        or "licensegenerator" in name
        or name == "signing-key.hex"
        or name.endswith((".pem", ".key", ".exe"))
        or (name.startswith("license-") and name.endswith(".txt"))
    ):
        blocked.append(path)
if blocked:
    print("Private/generated files must not be published:\n" + "\n".join(blocked), file=sys.stderr)
    sys.exit(1)
print("Public source tree contains no issuer tools or private key files.")
