'use strict';

const { accessSync, chmodSync, constants, statSync } = require('fs');
const { dirname, isAbsolute, join } = require('path');

const PLATFORMS = {
  'linux-x64': 'obu-linux-x64',
  'linux-arm64': 'obu-linux-arm64',
  'darwin-x64': 'obu-darwin-x64',
  'darwin-arm64': 'obu-darwin-arm64',
  'win32-x64': 'obu-windows-x64',
};

// Resolve beside this package, never through a shell or a desktop app's PATH.
// Both the CLI launcher and DSH use the very same optional binary dependency.
function resolveBinary(override) {
  let path = override;
  if (path !== undefined && (typeof path !== 'string' || !isAbsolute(path) || path.includes('\0'))) {
    throw new Error('obu: binaryPath must be an absolute executable path');
  }
  if (path === undefined) {
    const key = `${process.platform}-${process.arch}`;
    const pkg = PLATFORMS[key];
    if (!pkg) throw new Error(`obu: no prebuilt binary for ${key}. Build from source: https://github.com/aislopware/oni-browser-use#from-source`);
    try {
      const manifest = require.resolve(`${pkg}/package.json`);
      path = join(dirname(manifest), process.platform === 'win32' ? 'obu.exe' : 'obu');
    } catch {
      throw new Error(`obu: platform package "${pkg}" not installed. Reinstall obu with optional dependencies enabled.`);
    }
  }
  try {
    if (!statSync(path).isFile()) throw new Error('not a file');
    const mode = process.platform === 'win32' ? constants.F_OK : constants.X_OK;
    try {
      accessSync(path, mode);
    } catch (accessErr) {
      // Some tarball extractors / WSL setups strip the executable bit from
      // optional-dependency binaries. Attempt a safe in-place 0755 repair on
      // Unix before failing.
      if (process.platform !== 'win32' && accessErr && accessErr.code === 'EACCES') {
        try {
          chmodSync(path, 0o755);
          accessSync(path, mode);
        } catch {
          throw accessErr;
        }
      } else {
        throw accessErr;
      }
    }
  } catch (error) {
    throw new Error(`obu: cannot execute ${path}: ${error.message}. Reinstall obu or set binaryPath.`);
  }
  return path;
}

module.exports = { resolveBinary };
