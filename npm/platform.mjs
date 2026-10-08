const PLATFORMS = new Set(['linux', 'darwin', 'win32']);
const ARCHITECTURES = new Set(['x64', 'arm64']);

export function binaryName(platform = process.platform, architecture = process.arch) {
  if (!PLATFORMS.has(platform) || !ARCHITECTURES.has(architecture)) {
    throw new Error(`cannot run launchpad-cli on ${platform}-${architecture}; supported platforms are Linux, macOS and Windows on x64 or arm64`);
  }
  return `launchpad-cli-${platform}-${architecture}${platform === 'win32' ? '.exe' : ''}`;
}
