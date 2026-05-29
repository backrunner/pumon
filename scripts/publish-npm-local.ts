#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

type Options = {
  tag: string;
  yes: boolean;
  allowDirty: boolean;
  skipReleaseCheck: boolean;
  otp?: string;
};

type PackageJson = {
  name: string;
  version: string;
  repository?: {
    type?: string;
    url?: string;
    directory?: string;
  };
};

const here = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(here, "..");
const packageRoot = path.join(repoRoot, "packages", "pumon");
const packageJsonPath = path.join(packageRoot, "package.json");
const packageJson = JSON.parse(readFileSync(packageJsonPath, "utf8")) as PackageJson;
const options = parseArgs(process.argv.slice(2));

const expectedAssets = [
  `pumon-v${packageJson.version}-aarch64-apple-darwin.tar.gz`,
  `pumon-v${packageJson.version}-x86_64-unknown-linux-gnu.tar.gz`,
  `pumon-v${packageJson.version}-x86_64-pc-windows-msvc.zip`,
  `pumon-v${packageJson.version}-checksums.txt`
];

console.log(`Preparing ${packageJson.name}@${packageJson.version} for npm tag "${options.tag}".`);
console.log(options.yes ? "Mode: publish" : "Mode: dry-run. Pass --yes to publish.");

if (!options.allowDirty) {
  const status = command("git", ["status", "--porcelain"], { cwd: repoRoot, capture: true });
  if (status.stdout.trim()) {
    fail("git worktree is dirty. Commit/stash changes or pass --allow-dirty.");
  }
}

if (!packageJson.repository?.url?.includes("github.com/backrunner/pumon")) {
  fail("packages/pumon/package.json repository.url must point to github.com/backrunner/pumon.");
}

if (!options.skipReleaseCheck) {
  await assertReleaseAssets(packageJson.version);
}

command("npm", ["--prefix", "packages/node-support", "install"], { cwd: repoRoot });
command("npm", ["--prefix", "packages/node-support", "run", "build"], { cwd: repoRoot });
command("npm", ["--prefix", "packages/cluster-shim", "test"], { cwd: repoRoot });
command("cargo", ["build", "-p", "pumon-cli"], { cwd: repoRoot });
command("npm", ["test"], { cwd: packageRoot });
command("npm", ["pack", "--dry-run"], { cwd: packageRoot });

if (!options.yes) {
  console.log("\nDry-run complete.");
  console.log(`Publish with: node --experimental-strip-types scripts/publish-npm-local.ts --tag ${options.tag} --yes`);
  process.exit(0);
}

command("npm", ["whoami"], { cwd: packageRoot });

const publishArgs = ["publish", "--tag", options.tag];
if (options.otp) publishArgs.push(`--otp=${options.otp}`);
command("npm", publishArgs, { cwd: packageRoot });

console.log(`Published ${packageJson.name}@${packageJson.version} with dist-tag "${options.tag}".`);

function parseArgs(args: string[]): Options {
  const options: Options = {
    tag: "beta",
    yes: false,
    allowDirty: false,
    skipReleaseCheck: false
  };

  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--tag") {
      options.tag = valueAfter(args, ++index, "--tag");
    } else if (arg.startsWith("--tag=")) {
      options.tag = arg.slice("--tag=".length);
    } else if (arg === "--otp") {
      options.otp = valueAfter(args, ++index, "--otp");
    } else if (arg.startsWith("--otp=")) {
      options.otp = arg.slice("--otp=".length);
    } else if (arg === "--yes" || arg === "-y") {
      options.yes = true;
    } else if (arg === "--allow-dirty") {
      options.allowDirty = true;
    } else if (arg === "--skip-release-check") {
      options.skipReleaseCheck = true;
    } else if (arg === "--help" || arg === "-h") {
      printHelp();
      process.exit(0);
    } else {
      fail(`unknown argument: ${arg}`);
    }
  }

  if (!/^[a-z][a-z0-9._-]*$/i.test(options.tag)) {
    fail(`invalid npm dist-tag: ${options.tag}`);
  }

  return options;
}

function valueAfter(args: string[], index: number, flag: string): string {
  const value = args[index];
  if (!value || value.startsWith("-")) fail(`${flag} requires a value.`);
  return value;
}

async function assertReleaseAssets(version: string): Promise<void> {
  const releaseUrl = `https://api.github.com/repos/backrunner/pumon/releases/tags/v${version}`;
  console.log(`Checking GitHub Release assets for v${version}.`);
  const response = await fetch(releaseUrl, {
    headers: { Accept: "application/vnd.github+json" }
  });

  if (!response.ok) {
    fail(`GitHub Release v${version} is not available yet (${response.status}).`);
  }

  const release = await response.json() as { assets?: Array<{ name: string }> };
  const published = new Set((release.assets ?? []).map((asset) => asset.name));
  const missing = expectedAssets.filter((asset) => !published.has(asset));
  if (missing.length > 0) {
    fail(`GitHub Release v${version} is missing assets: ${missing.join(", ")}`);
  }
}

function command(
  executable: string,
  args: string[],
  options: { cwd: string; capture?: boolean }
): { stdout: string } {
  const result = spawnSync(executable, args, {
    cwd: options.cwd,
    encoding: "utf8",
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit"
  });

  if (result.status !== 0) {
    const commandText = [executable, ...args].join(" ");
    if (options.capture && result.stderr) process.stderr.write(result.stderr);
    fail(`command failed: ${commandText}`);
  }

  return { stdout: result.stdout ?? "" };
}

function printHelp(): void {
  console.log(`Usage:
  node --experimental-strip-types scripts/publish-npm-local.ts [options]

Options:
  --tag <tag>             npm dist-tag to publish with. Default: beta
  --yes, -y               Actually publish. Without this, the script only dry-runs.
  --otp <code>            Pass an npm 2FA one-time password to npm publish.
  --allow-dirty           Allow publishing from a dirty git worktree.
  --skip-release-check    Skip GitHub Release asset verification.
  --help, -h              Show this help.
`);
}

function fail(message: string): never {
  console.error(`\nerror: ${message}`);
  process.exit(1);
}
