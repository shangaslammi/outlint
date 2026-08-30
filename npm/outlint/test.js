"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
const test = require("node:test");
const packageJson = require("./package.json");
const {
  artifactFor,
  cacheRoot,
  cachedExecutable,
  checksumFromSidecar,
} = require("./install.js");

test("maps every release platform to its cargo-dist artifact", () => {
  assert.deepEqual(artifactFor("darwin", "arm64"), {
    target: "aarch64-apple-darwin",
    archive: "outlint-aarch64-apple-darwin.tar.xz",
    executable: "outlint",
  });
  assert.equal(artifactFor("darwin", "x64").target, "x86_64-apple-darwin");
  assert.equal(artifactFor("linux", "arm64", "gnu").target, "aarch64-unknown-linux-gnu");
  assert.equal(artifactFor("linux", "x64", "gnu").target, "x86_64-unknown-linux-gnu");
  assert.equal(artifactFor("linux", "x64", "musl").target, "x86_64-unknown-linux-musl");
  assert.deepEqual(artifactFor("win32", "x64"), {
    target: "x86_64-pc-windows-msvc",
    archive: "outlint-x86_64-pc-windows-msvc.zip",
    executable: "outlint.exe",
  });
});

test("rejects platforms for which no release artifact exists", () => {
  assert.throws(() => artifactFor("linux", "arm64", "musl"), /unsupported platform/);
  assert.throws(() => artifactFor("win32", "arm64"), /unsupported platform/);
  assert.throws(() => artifactFor("freebsd", "x64"), /unsupported platform/);
});

test("accepts cargo-dist checksum sidecars only for the selected archive", () => {
  const hash = "a".repeat(64);
  assert.equal(checksumFromSidecar(`${hash} *outlint-x.tar.xz\n`, "outlint-x.tar.xz"), hash);
  assert.equal(checksumFromSidecar(`${hash}  outlint-x.tar.xz\n`, "outlint-x.tar.xz"), hash);
  assert.throws(
    () => checksumFromSidecar(`${hash} *different.tar.xz\n`, "outlint-x.tar.xz"),
    /invalid checksum/,
  );
  assert.throws(() => checksumFromSidecar("not a checksum", "outlint-x.tar.xz"), /invalid checksum/);
});

test("the package does not depend on an install lifecycle script", () => {
  assert.equal(packageJson.scripts.postinstall, undefined);
  assert.equal(packageJson.scripts.install, undefined);
});

test("selects a per-user cache without relying on package-directory writes", () => {
  assert.equal(
    cacheRoot({ OUTLINT_CACHE_DIR: "relative-cache" }, "linux", "/home/me"),
    path.resolve("relative-cache"),
  );
  assert.equal(cacheRoot({ XDG_CACHE_HOME: "/cache" }, "linux", "/home/me"), "/cache/outlint");
  assert.equal(cacheRoot({}, "darwin", "/Users/me"), "/Users/me/Library/Caches/outlint");
  assert.equal(
    cacheRoot({ LOCALAPPDATA: "C:\\Users\\me\\cache" }, "win32", "C:\\Users\\me"),
    path.join("C:\\Users\\me\\cache", "outlint"),
  );
  assert.equal(
    cachedExecutable(
      "0.1.0",
      { target: "x86_64-unknown-linux-gnu", executable: "outlint" },
      "/cache/outlint",
    ),
    "/cache/outlint/0.1.0/x86_64-unknown-linux-gnu/outlint",
  );
});

test("a packed package installs without scripts and launches a cached binary", (context) => {
  let artifact;
  try {
    artifact = artifactFor(process.platform, process.arch);
  } catch (error) {
    context.skip(error.message);
    return;
  }

  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "outlint-npm-test-"));
  try {
    const npmCli = [
      path.join(path.dirname(process.execPath), "node_modules", "npm", "bin", "npm-cli.js"),
      path.join(
        path.dirname(process.execPath),
        "..",
        "lib",
        "node_modules",
        "npm",
        "bin",
        "npm-cli.js",
      ),
    ].find(fs.existsSync);
    if (!npmCli) {
      context.skip("could not locate the npm CLI next to Node.js");
      return;
    }
    let packOutput;
    try {
      packOutput = execFileSync(
        process.execPath,
        [npmCli, "pack", "--json", "--pack-destination", temporary, __dirname],
        { encoding: "utf8" },
      );
    } catch (error) {
      if (error.code === "EPERM") {
        context.skip("this sandbox does not permit Node.js to spawn npm");
        return;
      }
      throw error;
    }
    const [{ filename }] = JSON.parse(packOutput);
    const project = path.join(temporary, "project");
    fs.mkdirSync(project);
    execFileSync(
      process.execPath,
      [
        npmCli,
        "install",
        "--ignore-scripts",
        "--offline",
        "--no-audit",
        "--no-fund",
        path.join(temporary, filename),
      ],
      { cwd: project, stdio: "pipe" },
    );

    const installedPackage = JSON.parse(
      fs.readFileSync(path.join(project, "node_modules", "outlint", "package.json"), "utf8"),
    );
    assert.equal(installedPackage.scripts.postinstall, undefined);

    const cache = path.join(temporary, "cache");
    const binary = cachedExecutable(packageJson.version, artifact, cache);
    fs.mkdirSync(path.dirname(binary), { recursive: true });
    let argumentsToWrapper;
    let expectedOutput;
    if (process.platform === "win32") {
      fs.copyFileSync(process.execPath, binary);
      argumentsToWrapper = ["--version"];
      expectedOutput = process.version;
    } else {
      fs.writeFileSync(
        binary,
        `#!${process.execPath}\nconsole.log(JSON.stringify(process.argv.slice(2)));\n`,
        { mode: 0o755 },
      );
      argumentsToWrapper = ["first", "second"];
      expectedOutput = '["first","second"]';
    }

    const command = path.join(
      project,
      "node_modules",
      ".bin",
      process.platform === "win32" ? "outlint.cmd" : "outlint",
    );
    const output = execFileSync(command, argumentsToWrapper, {
      cwd: project,
      encoding: "utf8",
      env: { ...process.env, OUTLINT_CACHE_DIR: cache },
    });
    assert.equal(output.trim(), expectedOutput);
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
});
