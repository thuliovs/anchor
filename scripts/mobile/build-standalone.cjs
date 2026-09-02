#!/usr/bin/env node

const fs = require('node:fs');
const path = require('node:path');

const {
  EXPECTED_APPLICATION_ID,
  EXPECTED_BUNDLE_ENTRY,
  MOBILE_ANDROID_ROOT,
  assertFileExists,
  buildJdkEnvironment,
  computeSha256,
  ensureStandaloneBuildEnvironment,
  findNewestApk,
  formatBytes,
  listZipEntries,
  readApplicationIdFromApk,
  runGradleTask,
  verifyDebugSignature,
} = require('./standalone-utils.cjs');

try {
  const { apksignerPath, gradleWrapperPath, hermesBinaryPath, jdk, sdkPath } =
    ensureStandaloneBuildEnvironment();
  const buildEnv = buildJdkEnvironment(jdk);

  console.log('Environment OK');
  console.log(`java: ${jdk.javaVersion}`);
  console.log(`javac: ${jdk.javacVersion}`);
  console.log(`jar: ${jdk.jarVersion}`);
  console.log(`javaHome: ${buildEnv.JAVA_HOME}`);
  console.log(`javaPath: ${jdk.javaPath}`);
  console.log(`javacPath: ${jdk.javacPath}`);
  console.log(`jarPath: ${jdk.jarPath}`);
  console.log(`androidSdk: ${sdkPath}`);
  console.log(`gradleWrapper: ${gradleWrapperPath}`);
  console.log(`hermes: ${hermesBinaryPath}`);
  console.log(`apksigner: ${apksignerPath}`);

  runGradleTask(gradleWrapperPath, ':app:assembleStandalone', MOBILE_ANDROID_ROOT, buildEnv);

  const apkDirectory = path.join(
    MOBILE_ANDROID_ROOT,
    'app',
    'build',
    'outputs',
    'apk',
    'standalone',
  );
  const apkPath = findNewestApk(apkDirectory);
  if (apkPath === null) {
    throw new Error(
      'Standalone build finished without producing an APK under apps/mobile/android/app/build/outputs/apk/standalone.',
    );
  }

  assertFileExists(apkPath, `Expected APK not found at ${apkPath}.`);

  const entries = listZipEntries(apkPath, jdk.jarPath);
  if (!entries.includes(EXPECTED_BUNDLE_ENTRY)) {
    throw new Error(`APK does not contain ${EXPECTED_BUNDLE_ENTRY}. The standalone bundle was not embedded.`);
  }

  const applicationId = readApplicationIdFromApk(apkPath, sdkPath);
  if (applicationId !== EXPECTED_APPLICATION_ID) {
    throw new Error(
      `Unexpected application ID ${applicationId}. Expected ${EXPECTED_APPLICATION_ID} for the standalone artifact.`,
    );
  }

  verifyDebugSignature(apkPath, apksignerPath);

  const apkStats = fs.statSync(apkPath);
  const sha256 = computeSha256(apkPath);

  console.log('build:mobile:standalone OK');
  console.log(`apk: ${apkPath}`);
  console.log(`size: ${formatBytes(apkStats.size)}`);
  console.log(`sha256: ${sha256}`);
  console.log(`applicationId: ${applicationId}`);
  console.log(`bundle: ${EXPECTED_BUNDLE_ENTRY}`);
  console.log('signing: Android debug certificate for internal testing only');
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
