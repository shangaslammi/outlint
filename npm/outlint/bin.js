#!/usr/bin/env node

"use strict";

const { spawn } = require("node:child_process");
const { install } = require("./install.js");

async function main() {
  let binary;
  try {
    binary = await install();
  } catch (error) {
    console.error(`outlint: could not prepare the platform binary: ${error.message}`);
    process.exitCode = 2;
    return;
  }

  const child = spawn(binary, process.argv.slice(2), { stdio: "inherit" });

  child.on("error", (error) => {
    console.error(`outlint: could not start the platform binary: ${error.message}`);
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
}

main();
