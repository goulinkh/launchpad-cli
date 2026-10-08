import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { gunzipSync } from 'node:zlib';

// Verify the converter's exact bytes, not a locally regenerated interpretation.
const bytes = gunzipSync(await readFile(new URL('../openapi/launchpad.json.gz', import.meta.url)));
const provenance = JSON.parse(await readFile(new URL('../openapi/provenance.json', import.meta.url)));
const manifest = JSON.parse(await readFile(new URL('../package.json', import.meta.url)));
const lock = JSON.parse(await readFile(new URL('../package-lock.json', import.meta.url)));
assert.equal(provenance.converter, '@canonical/launchpad-openapi');
assert.equal(provenance.version, manifest.devDependencies[provenance.converter],
  'OpenAPI provenance does not match the pinned converter; regenerate the snapshot');
assert.equal(provenance.version, lock.packages[`node_modules/${provenance.converter}`].version,
  'OpenAPI provenance does not match the locked converter');
assert.equal(createHash('sha256').update(bytes).digest('hex'), provenance.sha256,
  'OpenAPI snapshot does not match provenance');
const document = JSON.parse(bytes);
assert.match(document.openapi, /^3\.0\./, 'The runtime supports OpenAPI 3.0');
assert.deepEqual(document.servers, provenance.servers);
