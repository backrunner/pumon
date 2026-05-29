# Release Channels

Promon uses SemVer tags and npm dist-tags:

- Stable: `v1.2.3`, npm `latest`.
- Beta: `v1.2.3-beta.1`, npm `beta`.
- Alpha: `v1.2.3-alpha.1`, npm `alpha`.

The npm wrapper resolves the package version to the matching GitHub Release tag, downloads the platform archive, verifies `promon-v<version>-checksums.txt`, caches the native binary, and executes it.

