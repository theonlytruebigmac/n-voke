#!/usr/bin/env node
// Keeps the app version identical in every file that carries it.
//
//   node scripts/version.mjs                 print the current version
//   node scripts/version.mjs patch           0.1.0 -> 0.1.1
//   node scripts/version.mjs minor           0.1.0 -> 0.2.0
//   node scripts/version.mjs major           0.1.0 -> 1.0.0
//   node scripts/version.mjs 1.2.3           set an exact version (1.2.3-1 for a pre-release)
//   node scripts/version.mjs --check         fail if the files disagree
//   node scripts/version.mjs --check --tag v1.2.3
//                                            also fail if the git tag doesn't match
//
// Needs no dependencies. See docs/RELEASING.md.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const SEMVER = /^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/;

const read = (rel) => readFileSync(join(root, rel), "utf8");
const write = (rel, text) => writeFileSync(join(root, rel), text);

/** Replaces the one capture group of `re` in `text`, failing if it doesn't match exactly once. */
function replaceOnce(text, re, version, where) {
  const flags = re.flags.includes("g") ? re.flags : re.flags + "g";
  const matches = [...text.matchAll(new RegExp(re.source, flags))];
  if (matches.length !== 1) throw new Error(`${where}: expected one version field, found ${matches.length}`);
  const [m] = matches;
  const start = m.index + m[0].indexOf(m[1]);
  return text.slice(0, start) + version + text.slice(start + m[1].length);
}

/** A Cargo.toml: the version in its [package] table. */
function cargoToml(file) {
  const re = /^\[package\][^[]*?^version\s*=\s*"([^"]+)"/ms;
  return { file, get: (t) => t.match(re)?.[1], set: (t, v) => replaceOnce(t, re, v, file) };
}

/** A Cargo.lock: the entry for one of this repo's own packages. */
function cargoLock(file, name) {
  const re = new RegExp(`^name = "${name}"\\nversion = "([^"]+)"`, "m");
  return { file, label: `${file} (${name})`, get: (t) => t.match(re)?.[1], set: (t, v) => replaceOnce(t, re, v, `${file} (${name})`) };
}

// Each target reads its current version and writes a new one without reformatting the file.
const targets = [
  {
    file: "package.json",
    get: (t) => JSON.parse(t).version,
    set: (t, v) => replaceOnce(t, /^ {2}"version": "([^"]+)"/m, v, "package.json"),
  },
  {
    file: "package-lock.json",
    get: (t) => {
      const lock = JSON.parse(t);
      if (lock.version !== lock.packages[""].version) return `${lock.version} / ${lock.packages[""].version}`;
      return lock.version;
    },
    set: (t, v) => {
      // The root version and the root package's version, both near the top of the file.
      const head = t.slice(0, t.indexOf('"node_modules/'));
      const rest = t.slice(head.length);
      const root = replaceOnce(head, /^ {2}"version": "([^"]+)"/m, v, "package-lock.json");
      return replaceOnce(root, /^ {6}"version": "([^"]+)"/m, v, "package-lock.json") + rest;
    },
  },
  cargoToml("src-tauri/Cargo.toml"),
  cargoLock("src-tauri/Cargo.lock", "n-voke"),
  cargoLock("src-tauri/Cargo.lock", "nvoke-core"),
  cargoToml("crates/core/Cargo.toml"),
  cargoToml("crates/tui/Cargo.toml"),
  cargoLock("crates/tui/Cargo.lock", "nvoke-tui"),
  cargoLock("crates/tui/Cargo.lock", "nvoke-core"),
  {
    file: "src-tauri/tauri.conf.json",
    get: (t) => JSON.parse(t).version,
    set: (t, v) => replaceOnce(t, /^ {2}"version": "([^"]+)"/m, v, "tauri.conf.json"),
  },
];

function current() {
  return targets.map((t) => ({ file: t.label ?? t.file, version: t.get(read(t.file)) }));
}

function fail(message) {
  console.error(`error: ${message}`);
  process.exit(1);
}

function bump(from, how) {
  const m = SEMVER.exec(from);
  if (!m) fail(`current version "${from}" is not a valid version`);
  const [major, minor, patch] = [Number(m[1]), Number(m[2]), Number(m[3])];
  if (how === "major") return `${major + 1}.0.0`;
  if (how === "minor") return `${major}.${minor + 1}.0`;
  return `${major}.${minor}.${patch + 1}`;
}

const args = process.argv.slice(2);
const tagIndex = args.indexOf("--tag");
const tag = tagIndex === -1 ? undefined : args[tagIndex + 1];
if (tagIndex !== -1 && !tag) fail("--tag needs a value, e.g. --tag v1.2.3");
const positional = args.filter((a, i) => !a.startsWith("--") && !(tagIndex !== -1 && i === tagIndex + 1));
const check = args.includes("--check");

if (args.includes("--help") || args.includes("-h")) {
  console.log(read("scripts/version.mjs").split("\n").slice(1, 13).map((l) => l.replace(/^\/\/ ?/, "")).join("\n"));
  process.exit(0);
}

const versions = current();
const distinct = new Set(versions.map((v) => v.version));

if (check) {
  if (distinct.size !== 1 || [...distinct][0] === undefined) {
    for (const v of versions) console.error(`  ${v.file.padEnd(40)} ${v.version}`);
    fail("version files disagree. Run `npm run version:bump -- <version>` to resync them.");
  }
  const version = [...distinct][0];
  if (!SEMVER.test(version)) fail(`"${version}" is not a valid version`);
  if (tag !== undefined && tag !== `v${version}`) fail(`tag ${tag} does not match the app version ${version} (expected v${version})`);
  console.log(`OK: all files are at ${version}${tag ? ` and match ${tag}` : ""}`);
  process.exit(0);
}

if (positional.length === 0) {
  for (const v of versions) console.log(`${v.file.padEnd(40)} ${v.version}`);
  process.exit(distinct.size === 1 ? 0 : 1);
}

if (positional.length > 1) fail("expected a single version argument");
const [request] = positional;
const from = versions[0].version;

let next;
if (["patch", "minor", "major"].includes(request)) next = bump(from, request);
else next = request.replace(/^v/, "");
if (!SEMVER.test(next)) fail(`"${request}" is not patch, minor, major, or a version like 1.2.3`);
if (next === from && distinct.size === 1) fail(`already at ${next}`);

// Windows MSI installers only accept a numeric pre-release identifier (1.2.3-1, not 1.2.3-beta.1).
const pre = SEMVER.exec(next)[4];
if (pre !== undefined && !/^\d+$/.test(pre)) {
  console.warn(`warning: the Windows MSI installer needs a numeric pre-release (e.g. ${next.split("-")[0]}-1); "${pre}" will fail the Windows build.`);
}

for (const t of targets) {
  const updated = t.set(read(t.file), next);
  write(t.file, updated);
  console.log(`${(t.label ?? t.file).padEnd(40)} ${t.get(read(t.file))}`);
}
console.log(`\nVersion is now ${next} (was ${from}). Review with git diff, then commit and tag v${next}.`);
