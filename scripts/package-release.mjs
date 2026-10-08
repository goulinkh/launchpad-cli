import { execFileSync } from 'node:child_process';
import { chmod, copyFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, win32 } from 'node:path';
import { pathToFileURL } from 'node:url';

import { binaryName } from '../npm/platform.mjs';
import { TARGETS, assetName, metadataFromFiles } from './release-metadata.mjs';

export function archiveInvocation(platform, archive, directory, stem, windowsRoot = process.env.SystemRoot ?? 'C:\\Windows') {
  if (platform === 'win32') {
    // Git Bash's GNU tar treats drive letters as remote hosts and cannot produce ZIPs.
    return { program: win32.join(windowsRoot, 'System32', 'tar.exe'), args: ['-a', '-cf', archive, '-C', directory, stem] };
  }
  return { program: 'tar', args: ['-czf', archive, '-C', directory, stem] };
}

export async function packageRelease(target, root = process.cwd()) {
  const platform = TARGETS[target];
  if (!platform) throw new Error(`unsupported release target: ${target}`);
  if (platform.platform !== process.platform || platform.architecture !== process.arch) {
    throw new Error('release packaging must smoke-test the binary on its native platform and architecture');
  }
  const { version } = await metadataFromFiles(process.env.RELEASE_TAG, root);
  const executable = `launchpad-cli${platform.extension}`;
  const source = join(root, 'target', target, 'release', executable);
  const actualVersion = execFileSync(source, ['--version'], { encoding: 'utf8', timeout: 30_000 }).trim();
  if (actualVersion !== `launchpad-cli ${version}`) throw new Error(`unexpected binary version: ${actualVersion}`);
  const schema = JSON.parse(execFileSync(source, ['schema', '--json'], { encoding: 'utf8', timeout: 30_000, maxBuffer: 4 * 1024 * 1024 }));
  if (!schema.ok || schema.schema_version !== 1 || !Array.isArray(schema.data?.commands) || schema.data.commands.length < 31) {
    throw new Error('binary did not expose the expected offline agent command schema');
  }
  const stem = `launchpad-cli-${version}-${platform.platform}-${platform.architecture}`;
  const temporary = await mkdtemp(join(tmpdir(), 'launchpad-cli-release-'));
  const outputDirectory = join(root, 'dist');
  const archive = join(outputDirectory, assetName(version, target));
  try {
    const contents = join(temporary, stem);
    await mkdir(join(contents, 'openapi'), { recursive: true });
    await mkdir(outputDirectory, { recursive: true });
    await copyFile(source, join(contents, executable));
    if (platform.platform !== 'win32') await chmod(join(contents, executable), 0o755);
    for (const file of ['README.md', 'NOTICE.md', 'LICENSE', 'openapi/CONVERTER-LICENSE']) {
      await copyFile(join(root, file), join(contents, file));
    }
    const { program, args } = archiveInvocation(platform.platform, archive, temporary, stem);
    execFileSync(program, args);
  } finally {
    await rm(temporary, { recursive: true, force: true });
  }
  const binaryDirectory = join(root, 'bin');
  await mkdir(binaryDirectory, { recursive: true });
  const npmBinary = join(binaryDirectory, binaryName(platform.platform, platform.architecture));
  await copyFile(source, npmBinary);
  if (platform.platform !== 'win32') await chmod(npmBinary, 0o755);
  return archive;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  console.log(await packageRelease(process.argv[2]));
}
