import { execFileSync } from 'node:child_process';
import { appendFile } from 'node:fs/promises';
import { pathToFileURL } from 'node:url';

import { metadataFromFiles, npmAssetName } from './release-metadata.mjs';

export async function detectRelease(before, root = process.cwd()) {
  if (typeof before !== 'string' || ![40, 64].includes(before.length) || !/^[0-9a-f]+$/i.test(before)) {
    throw new Error('before revision must be a full Git commit SHA');
  }
  const metadata = await metadataFromFiles(undefined, root);
  let previousVersion;
  if (!/^0+$/.test(before)) {
    const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_')));
    const options = { cwd: root, encoding: 'utf8', env, stdio: ['ignore', 'pipe', 'pipe'] };
    const path = execFileSync('git', ['ls-tree', '--name-only', before, '--', 'package.json'], options).trim();
    if (path) {
      previousVersion = JSON.parse(execFileSync('git', ['show', `${before}:package.json`], options)).version;
    }
  }
  return {
    changed: metadata.version !== previousVersion,
    ...metadata,
    npm_asset: npmAssetName(metadata.version),
  };
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const metadata = await detectRelease(process.env.BEFORE_SHA);
  if (process.env.GITHUB_OUTPUT) {
    const outputs = Object.entries(metadata).map(([key, value]) => `${key}=${value}\n`).join('');
    await appendFile(process.env.GITHUB_OUTPUT, outputs);
  }
  console.log(JSON.stringify(metadata));
}
