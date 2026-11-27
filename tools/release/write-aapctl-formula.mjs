// Write Formula/aapctl.rb, and the README section that names it, into a Homebrew tap checkout.
//
// Called by tools/release/update-homebrew-tap.mjs (the tap pull request) and by tools/release/homebrew-smoke.sh (a throwaway
// tap on a runner, which proves the four downloads install before the pull request is opened). The version and the four
// SHA-256 values are the release's; nothing here reads a network.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const BEGIN = "<!-- aapctl:begin -->";
export const END = "<!-- aapctl:end -->";
export const REPOSITORY = "vymalo/another-agentic-platform";

export function writeFormula({ tapDirectory, version, shas }) {
  if (!/^\d+\.\d+\.\d+$/.test(version ?? "")) throw new Error(`aapctl's version must be X.Y.Z, not '${version}'`);
  const keys = ["linuxX8664", "linuxAarch64", "darwinX8664", "darwinArm64"];
  for (const key of keys) {
    if (!/^[a-f0-9]{64}$/.test(shas?.[key] ?? "")) throw new Error(`${key} must be a SHA-256 of 64 lower-case hex characters`);
  }
  const release = `https://github.com/${REPOSITORY}/releases/download/v${version}`;
  const asset = (os, arch) => `${release}/aapctl_${version}_${os}_${arch}.tar.gz`;

  mkdirSync(join(tapDirectory, "Formula"), { recursive: true });
  writeFileSync(
    join(tapDirectory, "Formula", "aapctl.rb"),
    `class Aapctl < Formula
  desc "Command line of another-agentic-platform"
  homepage "https://github.com/${REPOSITORY}"
  version "${version}"
  license "MIT"

  on_macos do
    on_arm do
      url "${asset("darwin", "arm64")}"
      sha256 "${shas.darwinArm64}"
    end

    on_intel do
      url "${asset("darwin", "x86_64")}"
      sha256 "${shas.darwinX8664}"
    end
  end

  on_linux do
    on_arm do
      url "${asset("linux", "aarch64")}"
      sha256 "${shas.linuxAarch64}"
    end

    on_intel do
      url "${asset("linux", "x86_64")}"
      sha256 "${shas.linuxX8664}"
    end
  end

  def install
    bin.install "aapctl"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/aapctl --version")
  end
end
`,
  );

  // The README is prose, not a table: one marked section, replaced whole on every release and appended the first time, so
  // the tap's own text around it is never touched. The markers are the contract with this script; a half of one is refused
  // rather than guessed at.
  const readme = join(tapDirectory, "README.md");
  const section = `${BEGIN}
## aapctl

\`aapctl\` is the command line of [another-agentic-platform](https://github.com/${REPOSITORY}): it judges agent documents
offline (\`validate\`, \`render\`) and talks to the Platform API (\`get\`, \`apply\`, \`delete\`, \`login\`).

\`\`\`sh
brew install --formula vymalo/tap/aapctl
aapctl --version
\`\`\`

macOS (Apple Silicon and Intel) and Linux (arm64 and x86_64) are provided; the Linux binaries are static. Windows is not
distributed. Downloads come from [the platform's releases](https://github.com/${REPOSITORY}/releases) and are checked
against the formula's SHA-256 values.
${END}`;

  const text = existsSync(readme) ? readFileSync(readme, "utf8") : "# Vymalo Homebrew tap\n";
  const start = text.indexOf(BEGIN);
  const end = text.indexOf(END);
  if (start >= 0 && end > start) {
    writeFileSync(readme, `${text.slice(0, start)}${section}${text.slice(end + END.length)}`);
  } else if (start >= 0 || end >= 0) {
    throw new Error("README.md holds one of the aapctl markers and not the other: fix it by hand");
  } else {
    writeFileSync(readme, `${text.trimEnd()}\n\n${section}\n`);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [tapDirectory, version, linuxX8664, linuxAarch64, darwinX8664, darwinArm64] = process.argv.slice(2);
  if (!tapDirectory) {
    throw new Error(
      "usage: write-aapctl-formula.mjs <tap-dir> <version> <linux-x86_64> <linux-aarch64> <darwin-x86_64> <darwin-arm64>",
    );
  }
  writeFormula({ tapDirectory, version, shas: { linuxX8664, linuxAarch64, darwinX8664, darwinArm64 } });
}
