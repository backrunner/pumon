# Release Channels

Procwatch uses SemVer tags and npm dist-tags:

- Stable: `v1.2.3`, npm `latest`.
- Beta: `v1.2.3-beta.1`, npm `beta`.
- Alpha: `v1.2.3-alpha.1`, npm `alpha`.

The npm wrapper resolves the package version to the matching GitHub Release tag, downloads the platform archive, verifies `procwatch-v<version>-checksums.txt`, caches the native binary, and executes it.

The published npm package must also include the Node-side runtime assets Procwatch needs at execution time:

- `vendor/node-support/config-loader.js`
- `vendor/node-support/package-json.js`
- `vendor/node-support/protocol.js`
- `vendor/cluster-shim/index.js`

`packages/procwatch/scripts/sync-assets.mjs` populates these assets before `npm pack` and `npm publish`.
