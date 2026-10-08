import { createHash } from 'node:crypto';
import { readFile, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';

import { TARGETS, assetName, metadataFromFiles, npmAssetName } from './release-metadata.mjs';

export async function releaseChecksums(version, directory) {
  const expected = [...Object.keys(TARGETS).map(target => assetName(version, target)), npmAssetName(version)].sort();
  const actual = (await readdir(directory)).filter(name => name !== 'SHA256SUMS').sort();
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error('release assets must contain exactly one archive for each of the six supported targets and the npm package');
  }
  const entries = await Promise.all(expected.map(async name => {
    const bytes = await readFile(join(directory, name));
    if (!bytes.length) throw new Error(`empty release asset: ${name}`);
    return `${createHash('sha256').update(bytes).digest('hex')}  ${name}`;
  }));
  const checksums = entries.join('\n') + '\n';
  await writeFile(join(directory, 'SHA256SUMS'), checksums);
  return checksums;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const { version } = await metadataFromFiles(process.env.RELEASE_TAG);
  console.log(await releaseChecksums(version, join(process.cwd(), 'dist')));
}
