import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

import { detectRelease } from './detect-release.mjs';
import { archiveInvocation, packageRelease } from './package-release.mjs';
import { releaseChecksums } from './release-checksums.mjs';
import { binaryName } from '../npm/platform.mjs';
import { TARGETS, assetName, metadataFromFiles, npmAssetName, releaseMetadata } from './release-metadata.mjs';

async function fixture(run) {
  const root = await mkdtemp(join(tmpdir(), 'launchpad-cli-release-test-'));
  try { await run(root); } finally { await rm(root, { recursive: true, force: true }); }
}

async function manifests(root, version = '1.2.3') {
  await writeFile(join(root, 'Cargo.toml'), `[package]\nname = "launchpad-cli"\nversion = "${version}"\n\n[dependencies]\nserde = "1"\n`);
  await writeFile(join(root, 'package.json'), JSON.stringify({ name: '@goulin/launchpad-cli', version }));
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

function git(root, ...args) {
  const env = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('GIT_')));
  return execFileSync('git', args, { cwd: root, encoding: 'utf8', env, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

function initialiseGit(root) {
  git(root, 'init', '--quiet');
  git(root, 'config', 'user.name', 'Release Test');
  git(root, 'config', 'user.email', 'release@example.test');
  git(root, 'config', 'commit.gpgSign', 'false');
}

function commit(root) {
  git(root, 'add', '--all');
  git(root, 'commit', '--quiet', '-m', 'test fixture');
  return git(root, 'rev-parse', 'HEAD');
}

test('release detection skips other manifest changes and compares the pre-push revision', async () => {
  await fixture(async root => {
    initialiseGit(root);
    await manifests(root);
    const before = commit(root);
    await writeFile(join(root, 'package.json'), JSON.stringify({ name: '@goulin/launchpad-cli', version: '1.2.3', description: 'changed metadata' }));
    assert.equal((await detectRelease(before, root)).changed, false);
    await manifests(root, '1.2.4');
    const bumped = commit(root);
    await writeFile(join(root, 'package.json'), JSON.stringify({ name: '@goulin/launchpad-cli', version: '1.2.4', description: 'another commit in the push' }));
    commit(root);
    assert.deepEqual(await detectRelease(before, root), {
      changed: true, tag: 'v1.2.4', version: '1.2.4', prerelease: false, npm_asset: 'goulin-launchpad-cli-1.2.4.tgz',
    });
    assert.equal((await detectRelease(bumped, root)).changed, false);
  });
});

test('release detection handles initial pushes and absent prior manifests', async () => {
  await fixture(async root => {
    await manifests(root, '1.2.3-rc.1');
    for (const length of [40, 64]) {
      const result = await detectRelease('0'.repeat(length), root);
      assert.equal(result.changed, true);
      assert.equal(result.prerelease, true);
      assert.equal(result.tag, 'v1.2.3-rc.1');
    }
    initialiseGit(root);
    await writeFile(join(root, 'README.md'), 'initial repository');
    git(root, 'add', 'README.md');
    git(root, 'commit', '--quiet', '-m', 'initial fixture');
    const before = git(root, 'rev-parse', 'HEAD');
    assert.equal((await detectRelease(before, root)).changed, true);
  });
});

test('release detection rejects invalid baselines and inconsistent current manifests', async () => {
  await fixture(async root => {
    initialiseGit(root);
    await manifests(root);
    commit(root);
    for (const before of [undefined, '', 'HEAD', '--help', 'a'.repeat(39), `${'a'.repeat(40)}\n`]) {
      await assert.rejects(detectRelease(before, root), /full Git commit SHA/);
    }
    await assert.rejects(detectRelease('f'.repeat(40), root), /Command failed/);
    await writeFile(join(root, 'package.json'), '{"version":"1.2.4"}');
    await assert.rejects(detectRelease('0'.repeat(40), root), /must match/);
    await manifests(root, '1.2.3\nchanged=true');
    await assert.rejects(detectRelease('0'.repeat(40), root), /valid semantic version/);
  });
});

test('release detection emits validated GitHub job outputs', async () => {
  await fixture(async root => {
    await manifests(root);
    const output = join(root, 'github-output');
    execFileSync(process.execPath, [fileURLToPath(new URL('./detect-release.mjs', import.meta.url))], {
      cwd: root, encoding: 'utf8', env: { ...process.env, BEFORE_SHA: '0'.repeat(40), GITHUB_OUTPUT: output },
    });
    assert.equal(await readFile(output, 'utf8'), 'changed=true\ntag=v1.2.3\nversion=1.2.3\nprerelease=false\nnpm_asset=goulin-launchpad-cli-1.2.3.tgz\n');
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
    const names = [...Object.keys(TARGETS).map(target => assetName('1.2.3', target)), npmAssetName('1.2.3')];
    for (const name of names) await writeFile(join(root, name), `fixture for ${name}`);
    const checksums = await releaseChecksums('1.2.3', root);
    const hash = createHash('sha256').update(await readFile(join(root, names[0]))).digest('hex');
    assert.ok(checksums.includes(`${hash}  ${names[0]}\n`));
    assert.equal(checksums.trim().split('\n').length, 7);
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
    await writeFile(join(root, npmAssetName('1.2.3')), 'fixture npm package');
    await assert.rejects(releaseChecksums('1.2.3', root), /empty release asset/);
  });
});

test('Windows ZIP packaging selects native bsdtar independently of PATH', () => {
  const archive = 'C:\\build path\\release.zip';
  const directory = 'C:\\temporary path';
  assert.deepEqual(archiveInvocation('win32', archive, directory, 'release', 'D:\\Windows'), {
    program: 'D:\\Windows\\System32\\tar.exe', args: ['-a', '-cf', archive, '-C', directory, 'release'],
  });
  assert.deepEqual(archiveInvocation('linux', '/build/release.tar.gz', '/temporary', 'release'), {
    program: 'tar', args: ['-czf', '/build/release.tar.gz', '-C', '/temporary', 'release'],
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
    assert.equal(await readFile(join(root, 'bin', binaryName()), 'utf8'), await readFile(binary, 'utf8'));
    await writeFile(binary, '#!/usr/bin/env node\nconsole.log("launchpad-cli 0.0.0");\n');
    await assert.rejects(packageRelease(target, root), /unexpected binary version/);
  });
});
