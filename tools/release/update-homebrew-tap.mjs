// Propose the aapctl upgrade in vymalo/homebrew-tap: one bot branch, a monotonic version, one open pull request.
//
// Adapted from another-agentic-system's apps/tauri/scripts/update-homebrew-tap.mjs (read 2026-11-27). The differences: one
// formula and no cask (aapctl is a command line on both macOS and Linux), four downloads from this repository's releases, and
// a README section rather than a cask's. The credentials come from the environment, never from an argument.
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const branch = "bot/aapctl-release";
const repo = "vymalo/homebrew-tap";
const files = ["Formula/aapctl.rb", "README.md"];

export function gitAuthEnvironment(token) {
  if (!token) return {};
  return {
    GIT_CONFIG_COUNT: "1",
    GIT_CONFIG_KEY_0: "http.https://github.com/.extraheader",
    // Git smart HTTP requires Basic authentication, unlike GitHub's REST API.
    GIT_CONFIG_VALUE_0: `AUTHORIZATION: basic ${Buffer.from(`x-access-token:${token}`).toString("base64")}`,
  };
}

export function compareVersions(left, right) {
  const a = left.split(".").map(BigInt);
  const b = right.split(".").map(BigInt);
  for (let index = 0; index < 3; index++) {
    if (a[index] !== b[index]) return a[index] > b[index] ? 1 : -1;
  }
  return 0;
}

export function updateTap({ cwd, base, version, shas, run }) {
  if (!/^\d+\.\d+\.\d+$/.test(version) || shas.length !== 4 || !shas.every((sha) => /^[a-f0-9]{64}$/.test(sha))) {
    throw new Error("Expected a stable version and four SHA-256 checksums");
  }
  // The tap directory is handed to the injected command too, so a test can observe where a write would land.
  const execute = (command, args) =>
    run
      ? run(command, args, cwd)
      : execFileSync(command, args, {
          cwd,
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
          env: {
            ...process.env,
            // Pass credentials through the environment, never through logged command arguments.
            ...gitAuthEnvironment(process.env.GH_TOKEN),
          },
        }).trim();
  const git = (...args) => execute("git", args);
  git("check-ref-format", `refs/heads/${base}`);
  if (git("status", "--porcelain")) throw new Error("Tap checkout must be clean before updating");
  git("fetch", "origin", `+refs/heads/${base}:refs/remotes/origin/${base}`);
  const remoteHead = git("ls-remote", "--heads", "origin", `refs/heads/${branch}`).split(/\s/)[0];
  if (remoteHead) git("fetch", "origin", `+refs/heads/${branch}:refs/remotes/origin/${branch}`);
  const refs = [`origin/${base}`, ...(remoteHead ? [`origin/${branch}`] : [])];
  // Both refs are compared: the tap's default branch, and the bot branch a pending pull request lives on. A pending upgrade
  // is the version the tap is about to carry, so a release older than it is stale even though the default branch is older.
  for (const ref of refs) {
    for (const file of files) {
      if (!git("ls-tree", "--name-only", ref, "--", file)) continue;
      const text = git("show", `${ref}:${file}`);
      const existing = text.match(/version "(\d+\.\d+\.\d+)"/)?.[1] ?? text.match(/\/download\/v(\d+\.\d+\.\d+)\//)?.[1];
      // The README's section names no version, so a tap whose formula is older is the only thing that can be behind.
      if (!existing) continue;
      if (compareVersions(version, existing) < 0) return "stale";
    }
  }
  // Only this workflow owns the bot branch. Rebuild it from the fresh default branch, preserving unrelated tap changes, and
  // lease against the remote SHA observed above.
  git("checkout", "-B", branch, `origin/${base}`);
  execute(process.execPath, [fileURLToPath(new URL("./write-aapctl-formula.mjs", import.meta.url)), cwd, version, ...shas]);
  git("add", "--", ...files);
  if (!git("diff", "--cached", "--name-only", "--", ...files)) return "unchanged";
  const desiredTree = git("write-tree");
  const pendingTree = remoteHead ? git("rev-parse", `${remoteHead}^{tree}`) : "";
  if (desiredTree !== pendingTree) {
    git(
      "-c",
      "user.name=release-please[bot]",
      "-c",
      "user.email=release-please[bot]@users.noreply.github.com",
      "commit",
      "-m",
      `chore: update aapctl to ${version}`,
    );
    git("push", `--force-with-lease=refs/heads/${branch}:${remoteHead}`, "origin", `HEAD:refs/heads/${branch}`);
  } else {
    // The generated index is exactly the remote tree; keep its existing commit too.
    git("reset", "--soft", remoteHead);
  }
  const existing = JSON.parse(
    execute("gh", ["pr", "list", "--repo", repo, "--state", "open", "--head", branch, "--base", base, "--json", "url"]),
  );
  if (existing.length > 1) throw new Error("Multiple open tap update pull requests require manual reconciliation");
  const directory = mkdtempSync(join(tmpdir(), "aapctl-tap-pr-"));
  try {
    const bodyFile = join(directory, "body.md");
    const release = `https://github.com/vymalo/another-agentic-platform/releases/tag/v${version}`;
    const evidence = process.env.GITHUB_RUN_ID
      ? `https://github.com/vymalo/another-agentic-platform/actions/runs/${process.env.GITHUB_RUN_ID}`
      : "https://github.com/vymalo/another-agentic-platform/actions/workflows/release.yml";
    writeFileSync(
      bodyFile,
      `## 1. Summary

Update \`aapctl\` to ${version} from [the platform release](${release}), whose four binaries were built, signed and attested by that tag's run.

## 2. Intent

Distribute the command line of another-agentic-platform, at the same version as its images and charts, through Homebrew.

## 3. Scope

\`Formula/aapctl.rb\` (macOS Apple Silicon and Intel, Linux arm64 and x86_64) and the README's \`aapctl\` section. No cask, no Windows and no other package of this tap.

## 4. Verification

The calling [release workflow](${evidence}) installed the formula in a throwaway tap on each of the four platforms (\`brew install\`, \`aapctl --version\`, \`aapctl validate -f examples/coder.yaml\`) before this pull request was opened. The checksums come from the release's \`SHA256SUMS\`; the tarballs are signed with cosign (keyless) and carry a build provenance attestation, verifiable with \`cosign verify-blob\` and \`gh attestation verify\` (docs/guides/install-aapctl.md).

## 5. Screenshots / Evidence

[Release workflow](${evidence}) and [release assets](${release}).

## 6. Risk Assessment

Low: the formula is generated from the release's own checksums and the version only moves forward. Homebrew's audit is not run here; the four installation jobs are.

## 7. AI Usage Declaration

- [x] Generating the release automation implementation and tests

This update is mechanically generated; no human approval or runtime verification of the platform is implied.

## 8. Reviewer Focus

The version, the four checksums and their architecture mapping, and that the previous formula's version is lower.
`,
    );
    const options = ["--title", `chore: update aapctl to ${version}`, "--body-file", bodyFile];
    if (existing.length) {
      execute("gh", ["pr", "edit", existing[0].url, ...options]);
      return "updated";
    }
    execute("gh", ["pr", "create", "--repo", repo, "--base", base, "--head", branch, ...options]);
    return "created";
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.env.GITHUB_ACTIONS === "true" && process.env.GH_TOKEN) {
    // The App action masks the original token, not this encoded Git credential.
    const header = gitAuthEnvironment(process.env.GH_TOKEN).GIT_CONFIG_VALUE_0;
    console.log(`::add-mask::${header.slice("AUTHORIZATION: basic ".length)}`);
  }
  const [cwd, base, version, ...shas] = process.argv.slice(2);
  console.log(updateTap({ cwd: resolve(cwd), base, version, shas }));
}
