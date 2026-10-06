// Bumps the app version. Cargo.toml is the only place it lives: the exe gets it
// via CARGO_PKG_VERSION (updater, title bar, winres file version) and the
// installer gets it from scripts/release.mjs (/DAppVersion).
//
//   npm run bump -- patch|minor|major|<x.y.z>
//
// Commits "chore: release vX.Y.Z" and tags it; then `npm run release`.
import { readFileSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const RE = /(^version\s*=\s*")([^"]+)(")/m; // first `version =` is [package]
const run = (cmd, args) => {
  console.log(`[bump] run ${cmd} ${args.join(" ")}`);
  const r = spawnSync(cmd, args, { stdio: "inherit" });
  if (r.status !== 0) { console.error(`[bump] error: ${cmd} failed`); process.exit(1); }
};

const arg = process.argv[2];
if (!arg) { console.error("usage: npm run bump -- patch|minor|major|<x.y.z>"); process.exit(1); }
if (spawnSync("git", ["status", "--porcelain"], { encoding: "utf8" }).stdout.trim()) {
  console.error("[bump] error: working tree is not clean"); process.exit(1);
}

const text = readFileSync("Cargo.toml", "utf8");
const current = text.match(RE)[2];
const [maj, min, pat] = current.split(".").map(Number);
const next = arg === "major" ? `${maj + 1}.0.0`
  : arg === "minor" ? `${maj}.${min + 1}.0`
  : arg === "patch" ? `${maj}.${min}.${pat + 1}`
  : arg;
if (!/^\d+\.\d+\.\d+$/.test(next)) { console.error(`[bump] error: invalid version ${next}`); process.exit(1); }

writeFileSync("Cargo.toml", text.replace(RE, `$1${next}$3`));
console.log(`[bump] Cargo.toml: ${current} -> ${next}`);
run("cargo", ["update", "--workspace", "--offline"]); // refresh Cargo.lock's own entry
run("git", ["commit", "-qam", `chore: release v${next}`]);
run("git", ["tag", `v${next}`]);
console.log(`[bump] tagged v${next}. Next: npm run release -- --notes-file notes.md`);
