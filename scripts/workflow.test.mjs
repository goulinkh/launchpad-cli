import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import { parse } from 'yaml';

import { TARGETS } from './release-metadata.mjs';

const workflow = parse(await readFile(new URL('../.github/workflows/release.yml', import.meta.url), 'utf8'));
const { metadata, verify, build, release, publish } = workflow.jobs;

function action(job, name) {
  const steps = job.steps.filter(step => step.uses?.startsWith(`${name}@`));
  assert.equal(steps.length, 1, `expected one ${name} step`);
  return steps[0];
}

test('releases detect package version changes on main rather than tag pushes', () => {
  assert.deepEqual(workflow.on, { push: { branches: ['main'], paths: ['package.json'] } });
  assert.equal(workflow.concurrency['cancel-in-progress'], false);
  assert.equal(action(metadata, 'actions/checkout').with['fetch-depth'], 0);
  const detector = metadata.steps.find(step => step.id === 'package');
  assert.equal(detector.env.BEFORE_SHA, '${{ github.event.before }}');
  assert.equal(detector.run, 'node scripts/detect-release.mjs');
  for (const key of ['changed', 'tag', 'version', 'prerelease', 'npm_asset']) {
    assert.equal(metadata.outputs[key], `\${{ steps.package.outputs.${key} }}`);
  }
  assert.equal(verify.needs, 'metadata');
  assert.equal(verify.if, "needs.metadata.outputs.changed == 'true'");
  assert.deepEqual(build.needs, ['metadata', 'verify']);
  assert.deepEqual(release.needs, ['metadata', 'build']);
  assert.deepEqual(publish.needs, ['metadata', 'release']);
});

test('native builds preserve the complete target matrix and checked npm package', () => {
  const targets = Object.fromEntries(build.strategy.matrix.include.map(({ target, platform, architecture }) => [target, { platform, architecture }]));
  assert.deepEqual(targets, Object.fromEntries(Object.entries(TARGETS).map(([target, value]) => [target, { platform: value.platform, architecture: value.architecture }])));
  assert.equal(build.env.RELEASE_TAG, '${{ needs.metadata.outputs.tag }}');
  assert.ok(build.steps.some(step => step.run === 'node scripts/package-release.mjs "$RUST_TARGET"'));
  assert.match(action(build, 'actions/upload-artifact').with.path, /dist\/\*\s+bin\/\*/);
  assert.ok(release.steps.some(step => step.run === 'node scripts/package-npm.mjs'));
  assert.ok(release.steps.some(step => step.run === 'node scripts/release-checksums.mjs'));
  const upload = action(release, 'actions/upload-artifact').with;
  const download = action(publish, 'actions/download-artifact').with;
  assert.equal(upload.name, 'launchpad-cli-npm');
  assert.equal(upload.path, 'dist/${{ needs.metadata.outputs.npm_asset }}');
  assert.equal(upload['if-no-files-found'], 'error');
  assert.equal(download.name, upload.name);
  assert.equal(download.path, 'npm-dist');
});

test('release jobs keep credentials isolated and actions pinned to commit hashes', () => {
  assert.deepEqual(workflow.permissions, { contents: 'read' });
  assert.deepEqual(release.permissions, { contents: 'write' });
  assert.deepEqual(publish.permissions, { 'id-token': 'write' });
  for (const job of [metadata, verify, build]) assert.equal(job.permissions, undefined);
  for (const job of Object.values(workflow.jobs)) {
    for (const step of job.steps) {
      if (step.uses) assert.match(step.uses, /@[a-f0-9]{40}$/);
      if (step.uses?.startsWith('actions/checkout@')) {
        assert.equal(step.with['persist-credentials'], false);
        assert.equal(step.with.ref, '${{ github.sha }}');
      }
    }
  }
  assert.ok(!publish.steps.some(step => step.uses?.startsWith('actions/checkout@')));
  assert.doesNotMatch(JSON.stringify(publish), /NODE_AUTH_TOKEN|secrets\./);
  const node = action(publish, 'actions/setup-node').with;
  assert.equal(node['node-version'], 24);
  assert.equal(node['registry-url'], 'https://registry.npmjs.org');
  assert.equal(node['package-manager-cache'], false);
  assert.ok(publish.steps.some(step => step.run === 'npm install --global npm@11.19.1'));
});

test('automatic publication uses annotated immutable tags and prerelease npm dist-tags', () => {
  const github = release.steps.find(step => step.run?.includes('gh release create'));
  assert.match(github.run, /repos\/\$GH_REPO\/git\/tags/);
  assert.match(github.run, /object="\$GITHUB_SHA"/);
  assert.match(github.run, /repos\/\$GH_REPO\/git\/refs/);
  assert.match(github.run, /ref="refs\/tags\/\$RELEASE_TAG"/);
  assert.match(github.run, /--verify-tag/);
  assert.match(github.run, /--prerelease --latest=false/);
  assert.doesNotMatch(github.run, /--force|release (delete|edit)|--clobber/);
  const npm = publish.steps.find(step => step.run?.includes('npm publish'));
  assert.match(npm.run, /tag=latest/);
  assert.match(npm.run, /tag=next/);
  assert.match(npm.run, /npm publish "\.\/npm-dist\/\$NPM_ASSET" --access public --tag "\$tag"/);
  assert.equal(publish.env.PRERELEASE, '${{ needs.metadata.outputs.prerelease }}');
});

test('all release workflow shell commands parse without executing mutations', { skip: process.platform === 'win32' }, () => {
  for (const job of Object.values(workflow.jobs)) {
    for (const step of job.steps) {
      if (step.run) execFileSync('bash', ['-n'], { input: step.run, encoding: 'utf8' });
    }
  }
});
