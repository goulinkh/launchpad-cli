import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { chmod, copyFile, lstat, mkdir, mkdtemp, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import test from 'node:test';

import { binaryName } from '../npm/platform.mjs';
import { checkNpmPackage, packageNpm } from './package-npm.mjs';
import { TARGETS, npmAssetName } from './release-metadata.mjs';

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'launchpad cli npm test '));
  t.after(() => rm(root, { recursive: true, force: true }));
  for (const directory of ['bin', 'npm', 'openapi', 'src']) await mkdir(join(root, directory));
  const manifest = { ...JSON.parse(await readFile(new URL('../package.json', import.meta.url), 'utf8')), version: '1.2.3' };
  await writeFile(join(root, 'package.json'), JSON.stringify(manifest));
  await writeFile(join(root, 'Cargo.toml'), `[package]\nname = "launchpad-cli"\nversion = "${manifest.version}"\n`);
  for (const file of ['cli.mjs', 'platform.mjs']) {
    await copyFile(new URL(`../npm/${file}`, import.meta.url), join(root, 'npm', file));
  }
  for (const file of [
    'README.md', 'LICENSE', 'NOTICE.md', 'DEVELOPMENT.md', 'Cargo.lock', 'rust-toolchain.toml',
    'openapi/CONVERTER-LICENSE', 'openapi/launchpad.json.gz', 'openapi/provenance.json', 'src/main.rs',
  ]) {
    await writeFile(join(root, file), `fixture ${file}`);
  }
  return root;
}

async function binaries(root) {
  for (const target of Object.values(TARGETS)) {
    await writeFile(join(root, 'bin', binaryName(target.platform, target.architecture)), 'native binary fixture');
  }
}

async function hostBinary(root, body) {
  const executable = join(root, 'bin', binaryName());
  await writeFile(executable, `#!/usr/bin/env node\n${body}\n`);
  await chmod(executable, 0o755);
}

function npm(args, cwd) {
  const npmPath = process.env.npm_execpath;
  return execFileSync(npmPath ? process.execPath : 'npm', npmPath ? [npmPath, ...args] : args, {
    cwd, encoding: 'utf8', timeout: 120_000,
  });
}

test('npm binary selection covers exactly the native release matrix', () => {
  const names = Object.values(TARGETS).map(target => binaryName(target.platform, target.architecture));
  assert.equal(new Set(names).size, 6);
  assert.equal(names.filter(name => name.endsWith('.exe')).length, 2);
  for (const [platform, architecture] of [['freebsd', 'x64'], ['linux', 'ia32'], ['../../bin', 'arm64']]) {
    assert.throws(() => binaryName(platform, architecture), /cannot run launchpad-cli/);
  }
});

test('npm packaging refuses missing, empty, unexpected and non-regular binaries', async t => {
  const root = await fixture(t);
  await assert.rejects(checkNpmPackage(root), /exactly the six/);
  await binaries(root);
  const executable = join(root, 'bin', binaryName());
  await writeFile(executable, '');
  await assert.rejects(checkNpmPackage(root), /empty or non-regular/);
  await writeFile(executable, 'native binary fixture');
  await writeFile(join(root, 'bin', 'credentials.json'), 'must not be packaged');
  await assert.rejects(checkNpmPackage(root), /exactly the six/);
  await rm(join(root, 'bin', 'credentials.json'));
  await rm(executable);
  await mkdir(executable);
  await assert.rejects(checkNpmPackage(root), /empty or non-regular/);
  await rm(executable, { recursive: true });
  await rm(join(root, 'bin'), { recursive: true });
  await assert.rejects(checkNpmPackage(root), /without all six native binaries/);
});

test('npm packaging rejects symlinked binaries', { skip: process.platform === 'win32' }, async t => {
  const root = await fixture(t);
  await binaries(root);
  const executable = join(root, 'bin', binaryName());
  await rm(executable);
  await symlink(join(root, 'LICENSE'), executable);
  await assert.rejects(checkNpmPackage(root), /empty or non-regular/);
});

test('npm packaging enforces the scoped name and rejects runtime dependencies', async t => {
  const root = await fixture(t);
  await binaries(root);
  const path = join(root, 'package.json');
  const manifest = JSON.parse(await readFile(path, 'utf8'));
  for (const change of [{ name: 'launchpad-cli' }, { name: '@goulinkh/launchpad-cli' }, { private: true }]) {
    await writeFile(path, JSON.stringify({ ...manifest, ...change }));
    await assert.rejects(checkNpmPackage(root), /publishable as @goulin\/launchpad-cli/);
  }
  for (const change of [{ dependencies: { secret: '*' } }, { optionalDependencies: { secret: '*' } }]) {
    await writeFile(path, JSON.stringify({ ...manifest, ...change }));
    await assert.rejects(checkNpmPackage(root), /runtime dependencies/);
  }
});

test('npm launcher preserves stdin, stdout, stderr, arguments and exit codes', { skip: process.platform === 'win32' }, async t => {
  const root = await fixture(t);
  await hostBinary(root, `
    import { readFileSync } from 'node:fs';
    console.log(JSON.stringify({ args: process.argv.slice(2), input: readFileSync(0, 'utf8') }));
    console.error('native diagnostic');
    process.exit(5);
  `);
  const result = spawnSync(process.execPath, [join(root, 'npm', 'cli.mjs'), 'bug', 'view', 'a;$(not-a-command)', '--json'], {
    cwd: tmpdir(), encoding: 'utf8', input: '{"target":"1"}',
  });
  assert.equal(result.status, 5, result.stderr);
  assert.deepEqual(JSON.parse(result.stdout), { args: ['bug', 'view', 'a;$(not-a-command)', '--json'], input: '{"target":"1"}' });
  assert.equal(result.stderr, 'native diagnostic\n');
});

test('npm launcher reports missing binaries without mixing diagnostics into stdout', async t => {
  const root = await fixture(t);
  const result = spawnSync(process.execPath, [join(root, 'npm', 'cli.mjs'), '--version'], { encoding: 'utf8' });
  assert.equal(result.status, 1);
  assert.equal(result.stdout, '');
  assert.match(result.stderr, /reinstall @goulin\/launchpad-cli/);
});

test('npm launcher preserves native termination signals', { skip: process.platform === 'win32' }, async t => {
  const root = await fixture(t);
  await hostBinary(root, `process.kill(process.pid, 'SIGTERM');`);
  const result = spawnSync(process.execPath, [join(root, 'npm', 'cli.mjs')], { encoding: 'utf8' });
  assert.equal(result.signal, 'SIGTERM');
  assert.equal(result.stdout, '');
  assert.equal(result.stderr, '');
});

test('scoped npm tarball installs offline without scripts or Rust and includes GPL source without credentials', { skip: process.platform === 'win32' }, async t => {
  const root = await fixture(t);
  await binaries(root);
  await hostBinary(root, `console.log('launchpad-cli 1.2.3');`);
  for (const file of ['.development-instance.local.md', 'npm/credentials.json', 'openapi/credentials.json', 'src/credentials.json']) {
    await writeFile(join(root, file), 'must not be published');
  }
  const archive = await packageNpm(root);
  assert.equal(archive, join(root, 'dist', npmAssetName('1.2.3')));
  const names = execFileSync('tar', ['-tzf', archive], { encoding: 'utf8' }).trim().split('\n').sort();
  const expected = [
    'package/package.json', 'package/npm/cli.mjs', 'package/npm/platform.mjs',
    'package/README.md', 'package/LICENSE', 'package/NOTICE.md', 'package/DEVELOPMENT.md',
    'package/Cargo.toml', 'package/Cargo.lock', 'package/rust-toolchain.toml', 'package/src/main.rs',
    'package/openapi/CONVERTER-LICENSE', 'package/openapi/launchpad.json.gz', 'package/openapi/provenance.json',
    ...Object.values(TARGETS).map(target => `package/bin/${binaryName(target.platform, target.architecture)}`),
  ].sort();
  assert.deepEqual(names, expected);
  const manifest = JSON.parse(execFileSync('tar', ['-xOf', archive, 'package/package.json'], { encoding: 'utf8' }));
  assert.equal(manifest.name, '@goulin/launchpad-cli');
  assert.equal(manifest.version, '1.2.3');
  assert.deepEqual(manifest.bin, { 'launchpad-cli': 'npm/cli.mjs', lp: 'npm/cli.mjs', lpcli: 'npm/cli.mjs' });
  assert.equal(manifest.publishConfig.access, 'public');
  assert.equal(manifest.private, undefined);
  assert.equal(manifest.devDependencies, undefined);
  assert.equal(manifest.scripts, undefined);
  const install = join(root, 'install');
  npm(['install', '--prefix', install, '--ignore-scripts', '--offline', '--no-audit', '--no-fund', '--package-lock=false', archive], root);
  const packageRoot = join(install, 'node_modules', '@goulin', 'launchpad-cli');
  assert.equal((await lstat(join(packageRoot, 'bin', binaryName()))).mode & 0o111, 0o111);
  await assert.rejects(lstat(join(install, 'node_modules', '.bin', 'lpci')), { code: 'ENOENT' });
  for (const command of ['launchpad-cli', 'lp', 'lpcli']) {
    const launcher = join(install, 'node_modules', '.bin', command);
    assert.equal(execFileSync(launcher, ['--version'], { encoding: 'utf8' }), 'launchpad-cli 1.2.3\n');
  }
});
