"use strict";

const assert = require("node:assert/strict");
const test = require("node:test");
const { artifactFor, checksumFromSidecar } = require("./install.js");

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
