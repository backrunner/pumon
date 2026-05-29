# Procwatch Release Engineering

## Release Goals

Procwatch must be easy to install and run:

- Direct binary downloads from GitHub Releases.
- `npx procwatch` execution through npm.
- Separate stable, beta, and alpha channels.
- Repeatable release workflows.
- Clear artifact naming and checksum verification.

## Versioning

Use SemVer:

- Stable: `1.2.3`
- Beta: `1.2.3-beta.1`
- Alpha: `1.2.3-alpha.1`

Git tags:

- Stable: `v1.2.3`
- Beta: `v1.2.3-beta.1`
- Alpha: `v1.2.3-alpha.1`

Release branches:

- `main`: active development and alpha candidates.
- `beta`: beta stabilization.
- `release/*`: optional stable release preparation.

## Channels

### Stable

Purpose:

- Production-ready releases.

NPM:

- Package: `procwatch`
- Dist tag: `latest`

GitHub:

- Non-prerelease GitHub Release.

### Beta

Purpose:

- Feature-complete release candidates.

NPM:

- Package: `procwatch`
- Dist tag: `beta`

GitHub:

- Prerelease GitHub Release.

### Alpha

Purpose:

- Early integration testing.

NPM:

- Package: `procwatch`
- Dist tag: `alpha`

GitHub:

- Prerelease GitHub Release.

## Binary Artifacts

Recommended artifact names:

```text
procwatch-v1.2.3-x86_64-unknown-linux-gnu.tar.gz
procwatch-v1.2.3-aarch64-unknown-linux-gnu.tar.gz
procwatch-v1.2.3-x86_64-apple-darwin.tar.gz
procwatch-v1.2.3-aarch64-apple-darwin.tar.gz
procwatch-v1.2.3-x86_64-pc-windows-msvc.zip
procwatch-v1.2.3-checksums.txt
```

Each archive should contain:

- `procwatch` or `procwatch.exe`.
- `LICENSE`.
- `README.md`.
- Optional shell completions.

## GitHub Actions

### CI Workflow

Run on pull request and push:

- `cargo fmt --check`.
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `cargo test --workspace`.
- Node package lint/build for `packages/procwatch` and `packages/cluster-shim`.
- Node support package lint/build for JavaScript/TypeScript ecosystem config loading.
- Basic npm wrapper tests.

### Release Workflow

Triggered by tag:

1. Determine channel from tag suffix.
2. Build Rust binaries for all supported targets.
3. Build TypeScript packages.
4. Package artifacts.
5. Generate checksums.
6. Create GitHub Release.
7. Publish npm package with matching dist-tag.

### Nightly or Manual Alpha Workflow

Optional:

- Manual dispatch can publish alpha releases from `main`.
- Version must still be explicit to avoid accidental release overwrites.

## NPM Wrapper Behavior

`npx procwatch` should:

1. Execute `packages/procwatch/bin/procwatch.js`.
2. Detect platform and architecture.
3. Determine desired channel.
4. Find a local cached binary.
5. If missing, download from GitHub Releases.
6. Verify checksum.
7. Exec the binary and forward all arguments.

Channel resolution:

- `npx procwatch`: stable by default.
- `npx procwatch@beta`: beta through npm dist-tag.
- `npx procwatch@alpha`: alpha through npm dist-tag.
- `PROCWATCH_CHANNEL=beta npx procwatch`: explicit environment override, useful for testing.

The wrapper should support install scripts but must also work when npm lifecycle scripts are disabled.

## Binary Cache

Recommended cache location:

- macOS: `~/Library/Caches/procwatch/bin/`
- Linux: `~/.cache/procwatch/bin/`
- Windows: `%LOCALAPPDATA%\procwatch\Cache\bin\`

Cache key:

```text
<version>/<target-triple>/procwatch
```

The wrapper must not silently run a binary whose checksum does not match the release checksums.

## GitHub Release Lookup

Stable:

- Resolve the exact package version to GitHub tag `v<version>`.

Beta/alpha:

- Resolve exact npm package version to GitHub tag `v<version>`.

Avoid "latest release" lookup at runtime when npm already pins the package version. This makes `npx procwatch@1.2.3` deterministic.

## Package Versions

The npm package version should match the Procwatch binary version.

Examples:

- `procwatch@1.2.3` downloads `v1.2.3`.
- `procwatch@1.2.3-beta.1` downloads `v1.2.3-beta.1`.
- `procwatch@1.2.3-alpha.1` downloads `v1.2.3-alpha.1`.

## Security Considerations

Required:

- SHA256 checksum verification.
- TLS downloads only.
- No shell interpolation when executing downloaded binaries.
- Atomic binary writes to avoid corrupt cache entries.
- Clear error if checksum or download fails.

Recommended later:

- Sigstore/cosign signing.
- SLSA provenance.
- GitHub Artifact Attestations.

## Release Checklist

Before publishing stable:

- CI is green on all supported platforms.
- `procwatch doctor` passes on fresh machines.
- `npx procwatch --version` works on macOS, Linux, and Windows.
- Basic start/stop/logs flow works on each OS for CommonJS, ESM, TypeScript loader, TypeScript prebuilt output, and package manager script fixtures.
- Service install/uninstall is manually verified.
- Checksums are present and validated by npm wrapper.
- Docs mention any known platform limitations.

## MIT Open Source Requirements

Repository must include:

- `LICENSE` with MIT text.
- Clear copyright owner.
- Dependency license audit before stable release.
- Contributor guidelines can be added before public launch.
