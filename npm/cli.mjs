#!/usr/bin/env node
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

import { binaryName } from './platform.mjs';

function main() {
  let executable;
  try {
    executable = fileURLToPath(new URL(`../bin/${binaryName()}`, import.meta.url));
  } catch (error) {
    console.error(error.message);
    return 1;
  }
  const child = spawnSync(executable, process.argv.slice(2), { stdio: 'inherit' });
  if (child.error) {
    console.error(`cannot run bundled launchpad-cli: ${child.error.message}; reinstall @goulin/launchpad-cli`);
    return 1;
  }
  if (child.signal) {
    process.kill(process.pid, child.signal);
    return 1;
  }
  return child.status ?? 1;
}

process.exitCode = main();
