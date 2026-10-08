import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { packageRelease } from './package-release.mjs';
import { releaseChecksums } from './release-checksums.mjs';
import { TARGETS, assetName, metadataFromFiles, releaseMetadata } from './release-metadata.mjs';

async function fixture(run) {
  const root = await mkdtemp(join(tmpdir(), 'launchpad-cli-release-test-'));
  try { await run(root); } finally { await rm(root, { recursive: true, force: true }); }
}

async function manifests(root, version = '1.2.3') {
  await writeFile(join(root, 'Cargo.toml'), `[package]\nname = "launchpad-cli"\nversion = "${version}"\n\n[dependencies]\nserde = "1"\n`);
  await writeFile(join(root, 'package.json'), JSON.stringify({ name: 'launchpad-cli', version, private: true }));
}

test('release tags must match both manifests', () => {
  assert.deepEqual(releaseMetadata('v1.2.3', '1.2.3', '1.2.3'), { tag: 'v1.2.3', version: '1.2.3', prerelease: false });
  assert.equal(releaseMetadata('v1.2.3-rc.1', '1.2.3-rc.1', '1.2.3-rc.1').prerelease, true);
  assert.throws(() => releaseMetadata('v1.2.3', '1.2.4', '1.2.3'), /must match/);
  assert.throws(() => releaseMetadata('v1.2.3', '1.2.3', '1.2.4'), /must match/);
  for (const tag of ['1.2.3', 'v01.2.3', 'v1.2', 'v1.2.3-01', 'v1.2.3-rc..1', 'v1.2.3\nversion=injected', '../../v1.2.3']) {
    assert.throws(() => releaseMetadata(tag, tag.slice(1), tag.slice(1)), /valid semantic version/);
  }
});

test('reads package versions without resolving Cargo dependencies', async () => {
  await fixture(async root => {
    await manifests(root);
    assert.equal((await metadataFromFiles('v1.2.3', root)).version, '1.2.3');
    await writeFile(join(root, 'package.json'), '{"version":"2.0.0"}');
    await assert.rejects(metadataFromFiles('v1.2.3', root), /must match/);
  });
});

test('release target mapping contains exactly six unique native archives', () => {
  assert.equal(Object.keys(TARGETS).length, 6);
  const names = Object.keys(TARGETS).map(target => assetName('1.2.3', target));
  assert.equal(new Set(names).size, 6);
  assert.equal(names.filter(name => name.endsWith('.zip')).length, 2);
  assert.equal(names.filter(name => name.endsWith('.tar.gz')).length, 4);
  assert.throws(() => assetName('1.2.3', '../../malicious'), /unsupported/);
});

test('checksums require every target and reject unexpected artifacts', async () => {
  await fixture(async root => {
    const names = Object.keys(TARGETS).map(target => assetName('1.2.3', target));
    for (const name of names) await writeFile(join(root, name), `fixture for ${name}`);
    const checksums = await releaseChecksums('1.2.3', root);
    const hash = createHash('sha256').update(await readFile(join(root, names[0]))).digest('hex');
    assert.ok(checksums.includes(`${hash}  ${names[0]}\n`));
    assert.equal(checksums.trim().split('\n').length, 6);
    assert.equal(await readFile(join(root, 'SHA256SUMS'), 'utf8'), checksums);
    await writeFile(join(root, 'unexpected.tgz'), 'not a native release');
    await assert.rejects(releaseChecksums('1.2.3', root), /exactly one archive/);
    await rm(join(root, 'unexpected.tgz'));
    await rm(join(root, names[0]));
    await assert.rejects(releaseChecksums('1.2.3', root), /exactly one archive/);
  });
});

test('empty archives cannot be published', async () => {
  await fixture(async root => {
    for (const target of Object.keys(TARGETS)) await writeFile(join(root, assetName('1.2.3', target)), '');
    await assert.rejects(releaseChecksums('1.2.3', root), /empty release asset/);
  });
});

test('native packaging smoke-tests the executable and includes licences', { skip: process.platform === 'win32' }, async () => {
  await fixture(async root => {
    await manifests(root);
    const target = Object.keys(TARGETS).find(target => TARGETS[target].platform === process.platform && TARGETS[target].architecture === process.arch);
    assert.ok(target, 'tests require a supported native release platform');
    const binaryDirectory = join(root, 'target', target, 'release');
    await mkdir(binaryDirectory, { recursive: true });
    await mkdir(join(root, 'openapi'));
    for (const file of ['README.md', 'NOTICE.md', 'LICENSE', 'openapi/CONVERTER-LICENSE']) await writeFile(join(root, file), `fixture ${file}`);
    const binary = join(binaryDirectory, 'launchpad-cli');
    await writeFile(binary, '#!/usr/bin/env node\n' + `if (process.argv.includes('--version')) console.log('launchpad-cli 1.2.3'); else console.log(JSON.stringify({ok:true,schema_version:1,data:{commands:Array(31).fill({})}}));\n`);
    await chmod(binary, 0o755);
    const archive = await packageRelease(target, root);
    assert.ok(archive.endsWith(assetName('1.2.3', target)));
    const files = execFileSync('tar', ['-tzf', archive], { encoding: 'utf8' });
    assert.match(files, /\/launchpad-cli\n/);
    assert.match(files, /\/LICENSE\n/);
    assert.match(files, /\/openapi\/CONVERTER-LICENSE\n/);
    await writeFile(binary, '#!/usr/bin/env node\nconsole.log("launchpad-cli 0.0.0");\n');
    await assert.rejects(packageRelease(target, root), /unexpected binary version/);
  });
});
