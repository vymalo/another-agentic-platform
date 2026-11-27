// The tap tooling, without a tap, a network or Homebrew: the formula it writes, the README section it owns, the version order
// the monotonic check uses, and the three outcomes of an update. Run with
// `node --test tools/release/release.test.mjs` (the `scripts` job of operator-image.yml).
import { strict as assert } from "node:assert";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { BEGIN, END, writeFormula } from "./write-aapctl-formula.mjs";
import { compareVersions, updateTap } from "./update-homebrew-tap.mjs";

const shas = {
  linuxX8664: "a".repeat(64),
  linuxAarch64: "b".repeat(64),
  darwinX8664: "c".repeat(64),
  darwinArm64: "d".repeat(64),
};

function withTap(fn) {
  const directory = mkdtempSync(join(tmpdir(), "aapctl-tap-"));
  try {
    return fn(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

test("the formula names the four downloads of the release and their checksums", () =>
  withTap((tap) => {
    writeFormula({ tapDirectory: tap, version: "0.2.0", shas });
    const formula = readFileSync(join(tap, "Formula", "aapctl.rb"), "utf8");
    const base = "https://github.com/vymalo/another-agentic-platform/releases/download/v0.2.0/aapctl_0.2.0";
    assert.match(formula, /^class Aapctl < Formula$/m);
    assert.match(formula, /^  version "0\.2\.0"$/m);
    assert.match(formula, /^  license "MIT"$/m);
    for (const [asset, sha] of [
      ["linux_x86_64", shas.linuxX8664],
      ["linux_aarch64", shas.linuxAarch64],
      ["darwin_x86_64", shas.darwinX8664],
      ["darwin_arm64", shas.darwinArm64],
    ]) {
      assert.ok(formula.includes(`url "${base}_${asset}.tar.gz"`), `${asset}'s url is in the formula`);
      assert.ok(formula.includes(`sha256 "${sha}"`), `${asset}'s checksum is in the formula`);
    }
    // Each architecture sits in the block Homebrew selects it by, so an arm runner cannot install an x86_64 tarball.
    assert.match(formula, /on_macos do[\s\S]*on_arm do[\s\S]*darwin_arm64[\s\S]*on_intel do[\s\S]*darwin_x86_64[\s\S]*end\n  end/);
    assert.match(formula, /on_linux do[\s\S]*on_arm do[\s\S]*linux_aarch64[\s\S]*on_intel do[\s\S]*linux_x86_64[\s\S]*end\n  end/);
    assert.match(formula, /^    bin\.install "aapctl"$/m);
  }));

test("the README gets one section, and a second release replaces it rather than appending", () =>
  withTap((tap) => {
    writeFileSync(join(tap, "README.md"), "# Vymalo Homebrew tap\n\nThe desktop app lives below.\n");
    writeFormula({ tapDirectory: tap, version: "0.2.0", shas });
    writeFormula({ tapDirectory: tap, version: "0.3.0", shas });
    const readme = readFileSync(join(tap, "README.md"), "utf8");
    assert.equal(readme.split(BEGIN).length - 1, 1, "one section, however many releases wrote it");
    assert.equal(readme.split(END).length - 1, 1);
    assert.match(readme, /^# Vymalo Homebrew tap$/m, "the tap's own text is kept");
    assert.match(readme, /aapctl_0\.3\.0|## aapctl/, "the section is the new one");
    assert.match(readme, /brew install --formula vymalo\/tap\/aapctl/);
  }));

test("a README with half a marker is refused, never guessed at", () =>
  withTap((tap) => {
    writeFileSync(join(tap, "README.md"), `# Tap\n\n${BEGIN}\n## aapctl\n`);
    assert.throws(() => writeFormula({ tapDirectory: tap, version: "0.2.0", shas }), /one of the aapctl markers/);
  }));

test("a version that is not X.Y.Z, or a checksum that is not 64 hex characters, is refused", () =>
  withTap((tap) => {
    assert.throws(() => writeFormula({ tapDirectory: tap, version: "v0.2.0", shas }), /X\.Y\.Z/);
    assert.throws(
      () => writeFormula({ tapDirectory: tap, version: "0.2.0", shas: { ...shas, darwinArm64: "d".repeat(63) } }),
      /darwinArm64/,
    );
  }));

test("a tap is only moved forward", () => {
  assert.equal(compareVersions("0.3.0", "0.2.9"), 1);
  assert.equal(compareVersions("0.2.0", "0.2.0"), 0);
  assert.equal(compareVersions("0.2.0", "0.10.0"), -1);
  assert.equal(compareVersions("1.0.0", "0.99.99"), 1);
});

// A fake tap: the calls `updateTap` makes, up to the write, and the write itself. `changes` decides whether the generated
// files differ from what the tap holds. Every call carries the directory the command would run in, which is the tap's.
function fakeTap({ mainVersion, branchVersion, changes }) {
  const formula = (version) => `class Aapctl < Formula\n  version "${version}"\nend\n`;
  const calls = [];
  const run = (command, args, cwd) => {
    calls.push({ command, args, cwd });
    if (command === process.execPath) return "";
    if (command === "gh") {
      if (args[0] === "pr" && args[1] === "list") return "[]";
      if (args[0] === "pr" && args[1] === "create") return "https://github.com/vymalo/homebrew-tap/pull/1";
      throw new Error(`unexpected gh call: ${args.join(" ")}`);
    }
    const key = ["git", ...args].join(" ");
    if (key === "git check-ref-format refs/heads/main") return "";
    if (key === "git status --porcelain") return "";
    if (key === "git fetch origin +refs/heads/main:refs/remotes/origin/main") return "";
    if (key === "git ls-remote --heads origin refs/heads/bot/aapctl-release") {
      return branchVersion ? "deadbeef\trefs/heads/bot/aapctl-release" : "";
    }
    if (key === "git fetch origin +refs/heads/bot/aapctl-release:refs/remotes/origin/bot/aapctl-release") return "";
    if (key === "git ls-tree --name-only origin/main -- Formula/aapctl.rb") return "Formula/aapctl.rb";
    if (key === "git ls-tree --name-only origin/main -- README.md") return "";
    if (key === "git show origin/main:Formula/aapctl.rb") return formula(mainVersion);
    if (branchVersion && key === "git ls-tree --name-only origin/bot/aapctl-release -- Formula/aapctl.rb") {
      return "Formula/aapctl.rb";
    }
    if (branchVersion && key === "git ls-tree --name-only origin/bot/aapctl-release -- README.md") return "";
    if (branchVersion && key === "git show origin/bot/aapctl-release:Formula/aapctl.rb") return formula(branchVersion);
    if (key === "git checkout -B bot/aapctl-release origin/main") return "";
    if (key === "git add -- Formula/aapctl.rb README.md") return "";
    if (key === "git diff --cached --name-only -- Formula/aapctl.rb README.md") return changes ? "Formula/aapctl.rb" : "";
    if (key === "git write-tree") return "tree-of-the-upgrade";
    if (key === "git rev-parse deadbeef^{tree}") return "tree-of-the-open-branch";
    if (key.startsWith("git -c user.name=release-please[bot]") && args.includes("commit")) return "";
    if (key.startsWith("git push --force-with-lease=refs/heads/bot/aapctl-release:")) return "";
    throw new Error(`PROCEEDED past the version check: ${key}`);
  };
  return { run, calls };
}

const shasArray = [shas.linuxX8664, shas.linuxAarch64, shas.darwinX8664, shas.darwinArm64];

test("a release older than the tap's default branch is stale", () => {
  const tap = fakeTap({ mainVersion: "0.9.0", branchVersion: null });
  assert.equal(updateTap({ cwd: "/tap", base: "main", version: "0.3.0", shas: shasArray, run: tap.run }), "stale");
});

test("a release older than the open bot branch is stale, though the default branch is older still", () => {
  const tap = fakeTap({ mainVersion: "0.2.0", branchVersion: "0.9.0" });
  assert.equal(updateTap({ cwd: "/tap", base: "main", version: "0.3.0", shas: shasArray, run: tap.run }), "stale");
  assert.ok(
    tap.calls.some((call) => call.command === "git" && call.args.join(" ") === "show origin/bot/aapctl-release:Formula/aapctl.rb"),
    "the bot branch's formula is read, not only the default branch's",
  );
});

test("a release newer than both, whose files the tap already holds, is unchanged", () => {
  const tap = fakeTap({ mainVersion: "0.2.0", branchVersion: "0.2.0", changes: false });
  assert.equal(updateTap({ cwd: "/tap", base: "main", version: "0.3.0", shas: shasArray, run: tap.run }), "unchanged");
  assert.equal(tap.calls.filter((call) => call.command === "gh").length, 0, "an unchanged tap opens no pull request");
  const writer = tap.calls.find((call) => call.command === process.execPath);
  assert.ok(writer, "the formula writer runs");
  assert.equal(writer.cwd, "/tap", "the writer runs in the tap directory");
});

test("a release newer than both, whose files differ, is committed, pushed and proposed", () => {
  const tap = fakeTap({ mainVersion: "0.2.0", branchVersion: "0.2.0", changes: true });
  assert.equal(updateTap({ cwd: "/tap", base: "main", version: "0.3.0", shas: shasArray, run: tap.run }), "created");
  assert.ok(
    tap.calls.some((call) => call.command === "gh" && call.args[0] === "pr" && call.args[1] === "create"),
    "a new pull request is created",
  );
  assert.ok(
    tap.calls.some((call) => call.command === "git" && call.args.some((arg) => arg.startsWith("--force-with-lease=refs/heads/bot/aapctl-release:"))),
    "the bot branch is pushed under a lease",
  );
});
