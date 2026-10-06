// Builds, signs and publishes a release from this PC (same flow as nyaa-stream).
//
//   npm run release -- --notes-file notes.md [--dry-run] [--skip-build]
//   npm run release -- --notes "<one-line notes>" [--dry-run] [--skip-build]
//
// Expects HEAD to be the `chore: release vX.Y.Z` commit tagged vX.Y.Z (npm run bump).
// Output goes to target/installer: cutv-setup-X.Y.Z.exe, its .sig, and
// latest.json (what src/updater.rs reads). --dry-run builds and stages without
// pushing or publishing; --skip-build reuses the existing installer.
import { spawnSync } from "node:child_process";
import { createPrivateKey, sign } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const REPO = "sb-gravity100/cutv_rust";
const OUT = "target/installer";
const KEY_PATH = join(homedir(), ".cutv", "update-signing.pem");
const ISCC_CANDIDATES = [
  join(process.env.LOCALAPPDATA ?? "", "Programs", "Inno Setup 6", "ISCC.exe"),
  "C:\\Program Files (x86)\\Inno Setup 6\\ISCC.exe",
];

const log = (msg, extra) => console.log(`[release] ${msg}${extra ? " " + JSON.stringify(extra) : ""}`);
const fail = (msg) => { console.error(`[release] error: ${msg}`); process.exit(1); };

function parseArgs(argv) {
  const args = { notes: null, dryRun: false, skipBuild: false };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--dry-run") args.dryRun = true;
    else if (a === "--skip-build") args.skipBuild = true;
    else if (a === "--notes") args.notes = argv[++i] ?? null;
    else if (a === "--notes-file") {
      const file = argv[++i];
      if (!file || !existsSync(file)) fail(`notes file not found: ${file}`);
      args.notes = readFileSync(file, "utf8").trim();
      log("notes from file", { file, lines: args.notes.split(/\r?\n/).length });
    } else fail(`unknown argument: ${a}`);
  }
  if (!args.notes) fail('usage: npm run release -- --notes-file <file> | --notes "<text>" [--dry-run] [--skip-build]');
  return args;
}

/** Runs a command without a shell (args stay intact); exits on failure unless `allowFail`. */
function run(cmd, cmdArgs, { allowFail = false, inherit = false } = {}) {
  log("run", { cmd: [cmd, ...cmdArgs].join(" ") });
  const res = spawnSync(cmd, cmdArgs, { encoding: "utf8", stdio: inherit ? "inherit" : ["ignore", "pipe", "pipe"] });
  if (res.status !== 0 && !allowFail) {
    if (!inherit) console.error(res.stderr);
    fail(`command failed (${res.status}): ${cmd} ${cmdArgs[0] ?? ""}`);
  }
  return { ok: res.status === 0, out: (res.stdout ?? "").trim() };
}

function preflight(args) {
  log("preflight");
  const version = readFileSync("Cargo.toml", "utf8").match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (!version) fail("no version in Cargo.toml");
  const tag = `v${version}`;

  if (run("git", ["status", "--porcelain"]).out) fail("working tree is not clean");
  const headTags = run("git", ["tag", "--points-at", "HEAD"]).out.split(/\s+/);
  if (!headTags.includes(tag)) {
    const msg = `HEAD is not tagged ${tag} (run npm run bump)`;
    if (args.dryRun) log(`warning: ${msg} - fine for a dry run, required to publish`);
    else fail(msg);
  }
  const iscc = ISCC_CANDIDATES.find(existsSync);
  if (!args.skipBuild && !iscc) fail("Inno Setup not found (winget install JRSoftware.InnoSetup)");
  if (!existsSync(KEY_PATH)) fail(`signing key not found at ${KEY_PATH}`);
  if (!run("gh", ["auth", "status"], { allowFail: true }).ok) fail("gh is not logged in (gh auth login)");
  if (!args.dryRun && run("gh", ["release", "view", tag, "--repo", REPO], { allowFail: true }).ok) {
    fail(`release ${tag} is already published`);
  }
  log("preflight ok", { version, tag });
  return { version, tag, iscc };
}

function build({ version, iscc }) {
  log("building exe (cargo build --release)");
  run("cargo", ["build", "--release"], { inherit: true });
  log("building installer (ISCC)");
  run(iscc, ["/Q", `/DAppVersion=${version}`, "installer\\cutv.iss"], { inherit: true });
  log("build finished");
}

function stage({ version, tag }, notes) {
  const setup = `${OUT}/cutv-setup-${version}.exe`;
  if (!existsSync(setup)) fail(`missing build output: ${setup}`);
  // Ed25519 over the raw installer bytes; the key is never logged.
  const signature = sign(null, readFileSync(setup), createPrivateKey(readFileSync(KEY_PATH))).toString("base64");
  writeFileSync(`${setup}.sig`, signature + "\n");
  const latest = {
    version,
    notes,
    pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
    url: `https://github.com/${REPO}/releases/download/${tag}/cutv-setup-${version}.exe`,
    signature,
  };
  writeFileSync(`${OUT}/latest.json`, JSON.stringify(latest, null, 2) + "\n");
  const assets = [setup, `${setup}.sig`, `${OUT}/latest.json`];
  log("staged", { assets });
  return assets;
}

function publish({ tag }, notes, assets) {
  log("pushing main and the release tag", { tag });
  run("git", ["push", "origin", "HEAD:main"], { inherit: true });
  run("git", ["push", "origin", `refs/tags/${tag}`], { inherit: true });
  log("creating GitHub release", { tag });
  run("gh", ["release", "create", tag, "--repo", REPO, "--verify-tag", "--title", `CUTV ${tag}`, "--notes", notes, ...assets], { inherit: true });
  log("published", { url: `https://github.com/${REPO}/releases/tag/${tag}` });
}

const args = parseArgs(process.argv.slice(2));
log("start", { dryRun: args.dryRun, skipBuild: args.skipBuild });
const release = preflight(args);
if (args.skipBuild) log("skipping build, reusing installer");
else build(release);
const assets = stage(release, args.notes);
if (args.dryRun) log("dry run: not pushing or publishing");
else publish(release, args.notes, assets);
