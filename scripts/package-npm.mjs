import { execFileSync } from 'node:child_process';
import { chmod, copyFile, cp, lstat, mkdir, mkdtemp, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { pathToFileURL } from 'node:url';

import { binaryName } from '../npm/platform.mjs';
import { TARGETS, metadataFromFiles, npmAssetName } from './release-metadata.mjs';

const PACKAGE_FILES = [
  'npm/cli.mjs', 'npm/platform.mjs', 'README.md', 'NOTICE.md', 'LICENSE',
  'DEVELOPMENT.md', 'Cargo.toml', 'Cargo.lock', 'rust-toolchain.toml',
  'openapi/CONVERTER-LICENSE', 'openapi/launchpad.json.gz', 'openapi/provenance.json',
];

export async function checkNpmPackage(root = process.cwd()) {
  const { version } = await metadataFromFiles(process.env.RELEASE_TAG, root);
  const manifest = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
  if (manifest.name !== '@goulin/launchpad-cli' || manifest.private) {
    throw new Error('npm package must be publishable as @goulin/launchpad-cli');
  }
  if (Object.keys(manifest.dependencies ?? {}).length || Object.keys(manifest.optionalDependencies ?? {}).length) {
    throw new Error('npm package must not require runtime dependencies');
  }
  const names = Object.values(TARGETS).map(target => binaryName(target.platform, target.architecture)).sort();
  const directory = join(root, 'bin');
  let actual;
  try {
    actual = (await readdir(directory)).sort();
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
    throw new Error('cannot package npm release without all six native binaries; collect the release build artifacts first');
  }
  if (JSON.stringify(actual) !== JSON.stringify(names)) {
    throw new Error('npm release must contain exactly the six supported native binaries');
  }
  for (const name of names) {
    const path = join(directory, name);
    const metadata = await lstat(path);
    if (!metadata.isFile() || !metadata.size) throw new Error(`cannot package empty or non-regular binary: ${name}`);
    // GitHub artifact downloads do not preserve executable permissions.
    if (process.platform !== 'win32' && !name.endsWith('.exe')) await chmod(path, 0o755);
  }
  return { manifest, names, version };
}

export async function packageNpm(root = process.cwd()) {
  const { manifest, names, version } = await checkNpmPackage(root);
  const temporary = await mkdtemp(join(tmpdir(), 'launchpad-cli-npm-'));
  const outputDirectory = join(root, 'dist');
  try {
    const published = { ...manifest };
    delete published.scripts;
    delete published.devDependencies;
    delete published.private;
    published.files = [...PACKAGE_FILES, 'bin', 'src/**/*.rs'];
    await writeFile(join(temporary, 'package.json'), `${JSON.stringify(published, null, 2)}\n`);
    // Corresponding source must remain available even when the GitHub repository is private.
    await cp(join(root, 'src'), join(temporary, 'src'), { recursive: true });
    for (const file of [...PACKAGE_FILES, ...names.map(name => `bin/${name}`)]) {
      const destination = join(temporary, file);
      await mkdir(dirname(destination), { recursive: true });
      await copyFile(join(root, file), destination);
      if (process.platform !== 'win32' && (file === 'npm/cli.mjs' || (file.startsWith('bin/') && !file.endsWith('.exe')))) {
        await chmod(destination, 0o755);
      }
    }
    await mkdir(outputDirectory, { recursive: true });
    const args = ['pack', '--ignore-scripts', '--json', '--pack-destination', temporary];
    const npmPath = process.env.npm_execpath;
    if (process.platform === 'win32' && !npmPath) throw new Error('run npm packaging through npm run release:npm on Windows');
    const packs = JSON.parse(execFileSync(npmPath ? process.execPath : 'npm', npmPath ? [npmPath, ...args] : args, {
      cwd: temporary, encoding: 'utf8', timeout: 120_000,
    }));
    if (!Array.isArray(packs) || packs.length !== 1 || packs[0].filename !== npmAssetName(version)) {
      throw new Error('npm pack returned an unexpected package filename');
    }
    const archive = join(outputDirectory, npmAssetName(version));
    await copyFile(join(temporary, packs[0].filename), archive);
    return archive;
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  if (process.argv[2] === '--check') await checkNpmPackage();
  else console.log(await packageNpm());
}
