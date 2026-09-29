#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
target="${1:?Usage: bash scripts/build-unix.sh TARGET [OUTPUT_DIRECTORY]}"
output="${2:-dist}"
case "$target" in
  x86_64-apple-darwin|aarch64-apple-darwin|x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
  *) echo "Unsupported target: $target" >&2; exit 1 ;;
esac
test -f assets/license-public-key.hex
cargo fmt --check
cargo clippy --all-targets --locked --target "$target" -- -D warnings
cargo test --all-targets --locked --target "$target"
cargo build --release --locked --bin lan-print --target "$target"
if [[ -e "$output" ]]; then
  echo "Output directory already exists; choose a new empty path." >&2
  exit 1
fi
mkdir -p "$output"
cp "${CARGO_TARGET_DIR:-target}/$target/release/lan-print" "$output/LanPrint"
chmod +x "$output/LanPrint"
cp README.md "$output/"
cp -R docs "$output/"
(
  cd "$output"
  if command -v sha256sum >/dev/null 2>&1; then sha256sum LanPrint > SHA256SUMS.txt
  else shasum -a 256 LanPrint > SHA256SUMS.txt
  fi
)
echo "Ready: $output/LanPrint"
