#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');

const {
  MOBILE_ROOT,
  assertFileExists,
  createTempDir,
  ensureNodeVersion,
  formatBytes,
  removeDir,
  runCommand,
} = require('./standalone-utils.cjs');

const tempRoot = createTempDir('anchor-mobile-bundle-');
const bundleOutputPath = path.join(tempRoot, 'index.android.bundle');
const assetsOutputPath = path.join(tempRoot, 'assets');

try {
  ensureNodeVersion();

  assertFileExists(
    path.join(MOBILE_ROOT, 'index.js'),
    'Entry file apps/mobile/index.js not found. Restore it before running verify:mobile:bundle.',
  );
  assertFileExists(
    path.join(MOBILE_ROOT, 'metro.config.js'),
    'Metro config apps/mobile/metro.config.js not found. Restore it before running verify:mobile:bundle.',
  );

  const result = runCommand(
    process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm',
    [
      'exec',
      'react-native',
      'bundle',
      '--platform',
      'android',
      '--dev',
      'false',
      '--entry-file',
      'index.js',
      '--config',
      'metro.config.js',
      '--bundle-output',
      bundleOutputPath,
      '--assets-dest',
      assetsOutputPath,
      '--reset-cache',
    ],
    {
      cwd: MOBILE_ROOT,
      stdio: 'inherit',
      shell: process.platform === 'win32',
    },
  );

  if (result.status !== 0) {
    throw new Error('Metro bundle generation failed. Fix the error above and retry `pnpm verify:mobile:bundle`.');
  }

  assertFileExists(bundleOutputPath, 'Metro bundle command finished without writing index.android.bundle.');

  const bundleStats = fs.statSync(bundleOutputPath);
  if (bundleStats.size <= 0) {
    throw new Error('Generated Metro bundle is empty. Investigate Metro resolution before retrying.');
  }

  console.log('verify:mobile:bundle OK');
  console.log(`bundle: ${bundleOutputPath}`);
  console.log(`size: ${formatBytes(bundleStats.size)}`);
  console.log(`assets: ${assetsOutputPath}`);
  console.log('cleanup: temporary artifacts removed after verification');
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
} finally {
  removeDir(tempRoot);
}
