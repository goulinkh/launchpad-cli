import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFile, writeFile, rm } from 'node:fs/promises';
import { gzipSync } from 'node:zlib';

const output = new URL('../openapi/launchpad.json', import.meta.url);
const converter = new URL('../node_modules/@canonical/launchpad-openapi/dist/index.js', import.meta.url);
const result = spawnSync(process.execPath, [converter.pathname, '--format', 'json', '--output', output.pathname, ...process.argv.slice(2)], { stdio: 'inherit' });
if (result.status !== 0) process.exit(result.status ?? 1);
const bytes = await readFile(output);
const spec = JSON.parse(bytes);
await writeFile(new URL('../openapi/launchpad.json.gz', import.meta.url), gzipSync(bytes, { level: 9 }));
await writeFile(new URL('../openapi/provenance.json', import.meta.url), JSON.stringify({
  converter: '@canonical/launchpad-openapi', version: '0.0.3',
  source: 'https://code.launchpad.net/~launchpad-committers/launchpad-openapi/+git/launchpad-openapi',
  sha256: createHash('sha256').update(bytes).digest('hex'), servers: spec.servers,
}, null, 2) + '\n');
await rm(output);
