import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdtemp, mkdir, copyFile, readFile, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { test } from 'node:test';
import { gunzipSync } from 'node:zlib';

const name = '@canonical/launchpad-openapi';
const version = '0.0.99';
const bytes = JSON.stringify({
  openapi: '3.0.3', info: { title: 'Fixture', version: '1' },
  paths: {}, servers: [{ url: 'https://api.example.test/devel' }],
}) + '\n';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'openapi fixture '));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const directory of ['scripts', 'openapi', `node_modules/${name}/dist`]) {
    await mkdir(join(root, directory), { recursive: true });
  }
  for (const script of ['fetch-openapi.mjs', 'check-openapi.mjs']) {
    await copyFile(new URL(script, import.meta.url), join(root, 'scripts', script));
  }
  await writeFile(join(root, 'package.json'), JSON.stringify({ devDependencies: { [name]: version } }));
  await writeFile(join(root, 'package-lock.json'), JSON.stringify({
    packages: { [`node_modules/${name}`]: { version } },
  }));
  await writeFile(join(root, `node_modules/${name}/package.json`), JSON.stringify({ name, version, type: 'module' }));
  await writeFile(join(root, `node_modules/${name}/dist/index.js`), `
    import { writeFileSync } from 'node:fs';
    const output = process.argv[process.argv.indexOf('--output') + 1];
    writeFileSync(output, ${JSON.stringify(bytes)});
  `);
  return root;
}

function run(root, script) {
  return spawnSync(process.execPath, [join(root, 'scripts', script)], { encoding: 'utf8' });
}

test('snapshot generation records the installed pinned version and preserves exact converter bytes', async (t) => {
  const root = await fixture(t);
  const generated = run(root, 'fetch-openapi.mjs');
  assert.equal(generated.status, 0, generated.stderr);
  const provenance = JSON.parse(await readFile(join(root, 'openapi/provenance.json')));
  assert.equal(provenance.converter, name);
  assert.equal(provenance.version, version);
  assert.equal(gunzipSync(await readFile(join(root, 'openapi/launchpad.json.gz'))).toString(), bytes);
  await assert.rejects(readFile(join(root, 'openapi/launchpad.json')), { code: 'ENOENT' });
  const checked = run(root, 'check-openapi.mjs');
  assert.equal(checked.status, 0, checked.stderr);
});

test('snapshot generation refuses a stale installed converter', async (t) => {
  const root = await fixture(t);
  await writeFile(join(root, `node_modules/${name}/package.json`), JSON.stringify({ name, version: '0.0.1' }));
  const generated = run(root, 'fetch-openapi.mjs');
  assert.notEqual(generated.status, 0);
  assert.match(generated.stderr, /Installed converter does not match the exact pin/);
  await assert.rejects(readFile(join(root, 'openapi/launchpad.json.gz')), { code: 'ENOENT' });
});

test('snapshot checks reject stale provenance and a mismatched lockfile', async (t) => {
  const root = await fixture(t);
  assert.equal(run(root, 'fetch-openapi.mjs').status, 0);
  const path = join(root, 'openapi/provenance.json');
  const provenance = JSON.parse(await readFile(path));
  await writeFile(path, JSON.stringify({ ...provenance, version: '0.0.1' }));
  let checked = run(root, 'check-openapi.mjs');
  assert.notEqual(checked.status, 0);
  assert.match(checked.stderr, /provenance does not match the pinned converter/);
  await writeFile(path, JSON.stringify(provenance));
  await writeFile(join(root, 'package-lock.json'), JSON.stringify({
    packages: { [`node_modules/${name}`]: { version: '0.0.1' } },
  }));
  checked = run(root, 'check-openapi.mjs');
  assert.notEqual(checked.status, 0);
  assert.match(checked.stderr, /provenance does not match the locked converter/);
});
