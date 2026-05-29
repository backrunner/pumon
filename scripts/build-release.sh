#!/usr/bin/env bash
set -euo pipefail

version="${1:?version required}"
target="${2:?target required}"
archive="${3:?archive extension required}"
binary="target/${target}/release/promon"
dist="dist"
name="promon-v${version}-${target}"

mkdir -p "$dist/$name"
if [[ "$target" == *windows* ]]; then
  cp "${binary}.exe" "$dist/$name/promon.exe"
else
  cp "$binary" "$dist/$name/promon"
fi
cp LICENSE README.md "$dist/$name/"

if [[ "$archive" == "zip" ]]; then
  (cd "$dist" && zip -r "${name}.zip" "$name")
else
  (cd "$dist" && tar -czf "${name}.tar.gz" "$name")
fi
rm -rf "$dist/$name"

