#!/usr/bin/env node

"use strict";

const crypto = require("node:crypto");
const fs = require("node:fs");
const https = require("node:https");
const os = require("node:os");
const path = require("node:path");
const { execFileSync } = require("node:child_process");

const packageJson = require("./package.json");
const RELEASES = "https://github.com/shangaslammi/outlint/releases/download";

function linuxLibc() {
  const report = process.report?.getReport?.();
  if (report?.header?.glibcVersionRuntime) return "gnu";
  if (report?.sharedObjects?.some((library) => library.toLowerCase().includes("musl"))) return "musl";

  try {
    const output = execFileSync("ldd", ["--version"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    });
    const normalized = output.toLowerCase();
    if (normalized.includes("musl")) return "musl";
    if (normalized.includes("glibc") || normalized.includes("gnu libc")) return "gnu";
  } catch (error) {
    const output = `${error.stdout || ""}${error.stderr || ""}`;
    if (output.toLowerCase().includes("musl")) return "musl";
  }
  throw new Error("could not determine whether this Linux system uses glibc or musl");
}

function artifactFor(platform, arch, libc = platform === "linux" ? linuxLibc() : null) {
  const targets = {
    "darwin-arm64": "aarch64-apple-darwin",
    "darwin-x64": "x86_64-apple-darwin",
    "linux-arm64-gnu": "aarch64-unknown-linux-gnu",
    "linux-x64-gnu": "x86_64-unknown-linux-gnu",
    "linux-x64-musl": "x86_64-unknown-linux-musl",
    "win32-x64": "x86_64-pc-windows-msvc",
  };
  const key = platform === "linux" ? `${platform}-${arch}-${libc}` : `${platform}-${arch}`;
  const target = targets[key];
  if (!target) {
    throw new Error(`unsupported platform: ${platform}-${arch}${libc ? `-${libc}` : ""}`);
  }
  const extension = platform === "win32" ? "zip" : "tar.xz";
  return {
    target,
    archive: `outlint-${target}.${extension}`,
    executable: platform === "win32" ? "outlint.exe" : "outlint",
  };
}

function download(url, destination, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (redirects > 5) {
      reject(new Error(`too many redirects while downloading ${url}`));
      return;
    }

    const request = https.get(url, { headers: { "User-Agent": "outlint-npm-installer" } }, (response) => {
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        response.resume();
        const next = new URL(response.headers.location, url);
        if (next.protocol !== "https:") {
          reject(new Error(`refusing non-HTTPS redirect to ${next.href}`));
          return;
        }
        download(next.href, destination, redirects + 1).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        reject(new Error(`download failed with HTTP ${response.statusCode}: ${url}`));
        return;
      }

      const output = fs.createWriteStream(destination, { flags: "wx" });
      output.on("error", reject);
      response.on("error", reject);
      output.on("finish", () => output.close(resolve));
      response.pipe(output);
    });
    request.on("error", reject);
  });
}

function checksumFromSidecar(text, archive) {
  const match = text.trim().match(/^([0-9a-fA-F]{64}) [ *](.+)$/);
  if (!match || match[2] !== archive) {
    throw new Error(`invalid checksum file for ${archive}`);
  }
  return match[1].toLowerCase();
}

function sha256(file) {
  const hash = crypto.createHash("sha256");
  hash.update(fs.readFileSync(file));
  return hash.digest("hex");
}

async function install() {
  const artifact = artifactFor(process.platform, process.arch);
  const version = packageJson.version;
  const baseUrl = `${RELEASES}/v${version}/${artifact.archive}`;
  const temporary = fs.mkdtempSync(path.join(os.tmpdir(), "outlint-"));

  try {
    const archivePath = path.join(temporary, artifact.archive);
    const checksumPath = `${archivePath}.sha256`;
    await download(baseUrl, archivePath);
    await download(`${baseUrl}.sha256`, checksumPath);

    const expected = checksumFromSidecar(fs.readFileSync(checksumPath, "utf8"), artifact.archive);
    const actual = sha256(archivePath);
    if (actual !== expected) {
      throw new Error(`checksum mismatch for ${artifact.archive}`);
    }

    execFileSync("tar", ["-xf", archivePath, "-C", temporary], { stdio: "inherit" });
    const extracted = path.join(temporary, `outlint-${artifact.target}`, artifact.executable);
    const destinationDirectory = path.join(__dirname, "bin");
    const destination = path.join(destinationDirectory, artifact.executable);
    fs.mkdirSync(destinationDirectory, { recursive: true });
    fs.copyFileSync(extracted, destination);
    if (process.platform !== "win32") fs.chmodSync(destination, 0o755);
  } finally {
    fs.rmSync(temporary, { recursive: true, force: true });
  }
}

if (require.main === module) {
  install().catch((error) => {
    console.error(`outlint: failed to install the platform binary: ${error.message}`);
    process.exitCode = 1;
  });
}

module.exports = { artifactFor, checksumFromSidecar };
