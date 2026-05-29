#!/usr/bin/env bash
set -euo pipefail

version="${1:?version required}"
target="${2:?target required}"
archive="${3:?archive extension required}"
binary="target/${target}/release/pumon"
dist="dist"
name="pumon-v${version}-${target}"

mkdir -p "$dist/$name"
if [[ "$target" == *windows* ]]; then
  cp "${binary}.exe" "$dist/$name/pumon.exe"
else
  cp "$binary" "$dist/$name/pumon"
fi
cp LICENSE README.md "$dist/$name/"

if [[ "$archive" == "zip" ]]; then
  if command -v zip >/dev/null 2>&1; then
    (cd "$dist" && zip -r "${name}.zip" "$name")
  else
    powershell -NoProfile -Command \
      "Compress-Archive -Path '${dist}/${name}' -DestinationPath '${dist}/${name}.zip' -Force"
  fi
else
  (cd "$dist" && tar -czf "${name}.tar.gz" "$name")
fi
rm -rf "$dist/$name"
