import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile, writeFile, rm } from 'node:fs/promises';
import { gzipSync } from 'node:zlib';
import { fileURLToPath } from 'node:url';

const output = new URL('../openapi/launchpad.json', import.meta.url);
const converter = new URL('../node_modules/@canonical/launchpad-openapi/dist/index.js', import.meta.url);
const manifest = JSON.parse(await readFile(new URL('../package.json', import.meta.url)));
const installed = JSON.parse(await readFile(new URL('../node_modules/@canonical/launchpad-openapi/package.json', import.meta.url)));
assert.equal(installed.name, '@canonical/launchpad-openapi');
assert.equal(installed.version, manifest.devDependencies[installed.name],
  'Installed converter does not match the exact pin; run npm ci');
const result = spawnSync(process.execPath, [fileURLToPath(converter), '--format', 'json', '--output', fileURLToPath(output), ...process.argv.slice(2)], { stdio: 'inherit' });
if (result.status !== 0) process.exit(result.status ?? 1);
const bytes = await readFile(output);
const spec = JSON.parse(bytes);
await writeFile(new URL('../openapi/launchpad.json.gz', import.meta.url), gzipSync(bytes, { level: 9 }));
await writeFile(new URL('../openapi/provenance.json', import.meta.url), JSON.stringify({
  converter: installed.name, version: installed.version,
  source: 'https://code.launchpad.net/~launchpad-committers/launchpad-openapi/+git/launchpad-openapi',
  sha256: createHash('sha256').update(bytes).digest('hex'), servers: spec.servers,
}, null, 2) + '\n');
await rm(output);
