import { appendFile, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

export const TARGETS = Object.freeze({
  'x86_64-unknown-linux-musl': { platform: 'linux', architecture: 'x64', extension: '', archive: 'tar.gz' },
  'aarch64-unknown-linux-musl': { platform: 'linux', architecture: 'arm64', extension: '', archive: 'tar.gz' },
  'x86_64-apple-darwin': { platform: 'darwin', architecture: 'x64', extension: '', archive: 'tar.gz' },
  'aarch64-apple-darwin': { platform: 'darwin', architecture: 'arm64', extension: '', archive: 'tar.gz' },
  'x86_64-pc-windows-msvc': { platform: 'win32', architecture: 'x64', extension: '.exe', archive: 'zip' },
  'aarch64-pc-windows-msvc': { platform: 'win32', architecture: 'arm64', extension: '.exe', archive: 'zip' },
});

export function releaseMetadata(tag, cargoVersion, packageVersion) {
  const match = /^v((?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?)$/.exec(tag);
  if (!match || match[2]?.split('.').some(value => /^0\d+$/.test(value))) {
    throw new Error('release tag must be v followed by a valid semantic version');
  }
  const version = match[1];
  if (version !== cargoVersion || version !== packageVersion) {
    throw new Error(`tag ${tag} must match Cargo.toml (${cargoVersion}) and package.json (${packageVersion})`);
  }
  return { tag, version, prerelease: Boolean(match[2]) };
}

export async function metadataFromFiles(tag, root = process.cwd()) {
  const cargo = await readFile(join(root, 'Cargo.toml'), 'utf8');
  const section = /^\[package\]\s*\n([\s\S]*?)(?=^\[|(?![\s\S]))/m.exec(cargo)?.[1];
  const cargoVersion = /^version\s*=\s*"([^"]+)"\s*$/m.exec(section ?? '')?.[1];
  if (!cargoVersion) throw new Error('cannot read the package version from Cargo.toml');
  const manifest = JSON.parse(await readFile(join(root, 'package.json'), 'utf8'));
  return releaseMetadata(tag ?? `v${cargoVersion}`, cargoVersion, manifest.version);
}

export function assetName(version, target) {
  const metadata = TARGETS[target];
  if (!metadata) throw new Error(`unsupported release target: ${target}`);
  return `launchpad-cli-${version}-${metadata.platform}-${metadata.architecture}.${metadata.archive}`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const metadata = await metadataFromFiles(process.argv[2]);
  if (process.env.GITHUB_OUTPUT) {
    await appendFile(process.env.GITHUB_OUTPUT, `tag=${metadata.tag}\nversion=${metadata.version}\nprerelease=${metadata.prerelease}\n`);
  }
  console.log(JSON.stringify(metadata));
}
