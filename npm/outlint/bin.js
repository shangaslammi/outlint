#!/usr/bin/env node

"use strict";

const path = require("node:path");
const { spawn } = require("node:child_process");

const executable = process.platform === "win32" ? "outlint.exe" : "outlint";
const binary = path.join(__dirname, "bin", executable);
const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });

child.on("error", (error) => {
  if (error.code === "ENOENT") {
    console.error(
      "outlint: the platform binary is missing; reinstall the package without --ignore-scripts",
    );
  } else {
    console.error(`outlint: could not start the platform binary: ${error.message}`);
  }
  process.exitCode = 2;
});

child.on("exit", (code, signal) => {
  if (signal) {
    try {
      process.kill(process.pid, signal);
    } catch {
      process.exitCode = 1;
    }
  } else {
    process.exitCode = code === null ? 1 : code;
  }
});
