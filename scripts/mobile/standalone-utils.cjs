const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const REPO_ROOT = path.resolve(__dirname, '../..');
const MOBILE_ROOT = path.join(REPO_ROOT, 'apps', 'mobile');
const MOBILE_ANDROID_ROOT = path.join(MOBILE_ROOT, 'android');
const MOBILE_NODE_MODULES = path.join(MOBILE_ROOT, 'node_modules');
const EXPECTED_APPLICATION_ID = 'com.anchormobile.standalone';
const EXPECTED_BUNDLE_ENTRY = 'assets/index.android.bundle';

function parseJavaMajorVersion(output) {
  const match = output.match(/(?:version\s+"|javac\s+|jar\s+)(\d+)/i);
  if (match === null) {
    return null;
  }

  return Number.parseInt(match[1], 10);
}

function parseJavaHome(output) {
  const match = output.match(/^[ \t]*java\.home = (.+)$/m);
  if (match === null) {
    return null;
  }

  return match[1].trim();
}

function canonicalizePath(inputPath, platform = process.platform, realpath = fs.realpathSync.native) {
  const pathApi = platform === 'win32' ? path.win32 : path.posix;
  const resolvedPath = realpath(inputPath);
  const normalizedPath = pathApi.normalize(resolvedPath);

  if (platform === 'win32') {
    return normalizedPath.replace(/[\\/]+$/, '').replace(/\//g, '\\').toLowerCase();
  }

  return normalizedPath.length > 1 ? normalizedPath.replace(/\/+$/, '') : normalizedPath;
}

function getExecutableName(toolName, platform = process.platform) {
  return platform === 'win32' ? `${toolName}.exe` : toolName;
}

function getPathEntries(pathValue = process.env.PATH, platform = process.platform) {
  if (typeof pathValue !== 'string' || pathValue.length === 0) {
    return [];
  }

  return pathValue.split(platform === 'win32' ? ';' : ':').filter((entry) => entry.length > 0);
}

function readCommandVersion(execFile, commandPath, args) {
  const result = execFile(commandPath, args);
  const versionText = `${result.stdout ?? ''}\n${result.stderr ?? ''}`.trim();
  if (result.status !== 0) {
    throw new Error(`Command failed: ${commandPath} ${args.join(' ')}`);
  }

  return versionText;
}

function validateResolvedJdkTools({ javaPath, javacPath, jarPath, javaHome, execFile, platform, realpath }) {
  const javaVersion = readCommandVersion(execFile, javaPath, ['-XshowSettings:properties', '-version']);
  const javacVersion = readCommandVersion(execFile, javacPath, ['-version']);
  const jarVersion = readCommandVersion(execFile, jarPath, ['--version']);

  const javaMajor = parseJavaMajorVersion(javaVersion);
  const javacMajor = parseJavaMajorVersion(javacVersion);
  const jarMajor = parseJavaMajorVersion(jarVersion);
  const reportedJavaHome = parseJavaHome(javaVersion);
  const canonicalExpectedJavaHome = canonicalizePath(javaHome, platform, realpath);
  const canonicalReportedJavaHome =
    reportedJavaHome === null ? null : canonicalizePath(reportedJavaHome, platform, realpath);
  if (canonicalReportedJavaHome === null || canonicalReportedJavaHome !== canonicalExpectedJavaHome) {
    throw new Error(
      `JDK resolution mismatch. Expected java.home ${canonicalExpectedJavaHome} but java reported ${canonicalReportedJavaHome ?? 'unknown'}. Fix JAVA_HOME/PATH so java, javac and jar come from the same JDK 21 installation.`,
    );
  }

  if (javaMajor !== 21 || javacMajor !== 21 || jarMajor !== 21) {
    const javaHomeText = javaHome === null ? 'unset' : javaHome;
    throw new Error(
      `JDK 21 is required but resolved JAVA_HOME=${javaHomeText} with java \`${javaVersion || 'unknown version'}\`, javac \`${javacVersion || 'unknown version'}\` and jar \`${jarVersion || 'unknown version'}\`. Install JDK 21 or set JAVA_HOME to a JDK 21 installation and retry.`,
    );
  }

  return {
    jarPath,
    jarVersion,
    javaHome: canonicalExpectedJavaHome,
    reportedJavaHome: canonicalReportedJavaHome,
    javaPath,
    javaVersion,
    javacPath,
    javacVersion,
  };
}

function resolveJdkTools({
  env = process.env,
  execFile = runCommand,
  fileExists = fs.existsSync,
  pathEntries = getPathEntries(env.PATH, process.platform),
  platform = process.platform,
  realpath = fs.realpathSync.native,
} = {}) {
  const pathApi = platform === 'win32' ? path.win32 : path.posix;
  const binDirName = 'bin';
  const javaName = getExecutableName('java', platform);
  const javacName = getExecutableName('javac', platform);
  const jarName = getExecutableName('jar', platform);
  const javaHome = typeof env.JAVA_HOME === 'string' && env.JAVA_HOME.length > 0 ? env.JAVA_HOME : null;

  if (javaHome !== null) {
    const javaPath = pathApi.join(javaHome, binDirName, javaName);
    const javacPath = pathApi.join(javaHome, binDirName, javacName);
    const jarPath = pathApi.join(javaHome, binDirName, jarName);
    if (!fileExists(javaPath) || !fileExists(javacPath) || !fileExists(jarPath)) {
      throw new Error(
        `JAVA_HOME=${javaHome} does not point to a complete JDK installation. Expected ${javaPath}, ${javacPath} and ${jarPath}. Install JDK 21 or fix JAVA_HOME and retry.`,
      );
    }

    return validateResolvedJdkTools({
      javaPath,
      javacPath,
      jarPath,
      javaHome,
      execFile,
      platform,
      realpath,
    });
  }

  for (const entry of pathEntries) {
    const javaShimPath = pathApi.join(entry, javaName);
    if (!fileExists(javaShimPath)) {
      continue;
    }

    try {
      const javaVersion = readCommandVersion(execFile, javaShimPath, ['-XshowSettings:properties', '-version']);
      const javaHome = parseJavaHome(javaVersion);
      if (javaHome === null) {
        continue;
      }

      const javaPath = pathApi.join(javaHome, binDirName, javaName);
      const javacPath = pathApi.join(javaHome, binDirName, javacName);
      const jarPath = pathApi.join(javaHome, binDirName, jarName);
      if (!fileExists(javaPath) || !fileExists(javacPath) || !fileExists(jarPath)) {
        continue;
      }

      return validateResolvedJdkTools({
        javaPath,
        javacPath,
        jarPath,
        javaHome,
        execFile,
        platform,
        realpath,
      });
    } catch (error) {
      continue;
    }
  }

  throw new Error(
    'JDK 21 is required but no valid JDK was found via JAVA_HOME or PATH. Install JDK 21 or set JAVA_HOME to a JDK 21 installation and retry.',
  );
}

function buildJdkEnvironment(jdk, env = process.env, platform = process.platform) {
  const nextEnv = { ...env };
  const jdkBinDir = path.dirname(jdk.javaPath);
  const separator = platform === 'win32' ? ';' : ':';
  const existingEntries = getPathEntries(env.PATH, platform).filter((entry) => entry !== jdkBinDir);

  nextEnv.JAVA_HOME = jdk.javaHome;
  nextEnv.PATH = [jdkBinDir, ...existingEntries].join(separator);
  return nextEnv;
}

function detectHermesOsBin(platform = process.platform, arch = process.arch) {
  if (platform === 'win32') {
    if (arch === 'x64' || arch === 'arm64') {
      return 'win64-bin';
    }
  }

  if (platform === 'darwin') {
    if (arch === 'x64' || arch === 'arm64') {
      return 'osx-bin';
    }
  }

  if (platform === 'linux') {
    if (arch === 'x64') {
      return 'linux64-bin';
    }
  }

  throw new Error(`Unsupported platform for hermes-compiler: ${platform} ${arch}`);
}

function getHermesExecutableName(platform = process.platform) {
  return platform === 'win32' ? 'hermesc.exe' : 'hermesc';
}

function resolveAndroidSdkPath(env, localPropertiesText) {
  if (typeof env.ANDROID_HOME === 'string' && env.ANDROID_HOME.length > 0) {
    return env.ANDROID_HOME;
  }

  if (typeof env.ANDROID_SDK_ROOT === 'string' && env.ANDROID_SDK_ROOT.length > 0) {
    return env.ANDROID_SDK_ROOT;
  }

  if (typeof localPropertiesText === 'string') {
    const match = localPropertiesText.match(/^sdk\.dir=(.+)$/m);
    if (match !== null) {
      return match[1].trim().replace(/\\:/g, ':').replace(/\\\\/g, '\\');
    }
  }

  return null;
}

function readLocalProperties(androidRoot = MOBILE_ANDROID_ROOT) {
  const localPropertiesPath = path.join(androidRoot, 'local.properties');
  if (!fs.existsSync(localPropertiesPath)) {
    return null;
  }

  return fs.readFileSync(localPropertiesPath, 'utf8');
}

function ensureNodeVersion() {
  const major = Number.parseInt(process.versions.node.split('.')[0], 10);
  if (Number.isNaN(major) || major < 20) {
    throw new Error(
      `Node ${process.versions.node} is incompatible. Use Node 20+ before running this command.`,
    );
  }
}

function runCommand(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: 'utf8',
    stdio: options.stdio ?? 'pipe',
    cwd: options.cwd,
    shell: options.shell ?? false,
    env: options.env,
  });

  if (result.error) {
    throw result.error;
  }

  return result;
}

function ensureGradleWrapper(androidRoot = MOBILE_ANDROID_ROOT) {
  const wrapperName = process.platform === 'win32' ? 'gradlew.bat' : 'gradlew';
  const wrapperPath = path.join(androidRoot, wrapperName);
  if (!fs.existsSync(wrapperPath)) {
    throw new Error(
      `Gradle wrapper not found at ${wrapperPath}. Restore the wrapper before running this command.`,
    );
  }

  return wrapperPath;
}

function ensureAndroidSdk(androidRoot = MOBILE_ANDROID_ROOT) {
  const sdkPath = resolveAndroidSdkPath(process.env, readLocalProperties(androidRoot));
  if (sdkPath === null) {
    throw new Error(
      'Android SDK not found. Set ANDROID_HOME or ANDROID_SDK_ROOT, or configure sdk.dir in apps/mobile/android/local.properties.',
    );
  }

  if (!fs.existsSync(sdkPath)) {
    throw new Error(
      `Android SDK path does not exist: ${sdkPath}. Fix ANDROID_HOME/ANDROID_SDK_ROOT or apps/mobile/android/local.properties and retry.`,
    );
  }

  return sdkPath;
}

function getHermesBinaryPath(platform = process.platform, arch = process.arch) {
  return path.join(
    MOBILE_NODE_MODULES,
    'hermes-compiler',
    'hermesc',
    detectHermesOsBin(platform, arch),
    getHermesExecutableName(platform),
  );
}

function ensureHermesBinary() {
  const hermesBinaryPath = getHermesBinaryPath();
  if (!fs.existsSync(hermesBinaryPath)) {
    throw new Error(
      `Direct hermes-compiler binary not found at ${hermesBinaryPath}. Run \`pnpm install\` and ensure anchor-mobile declares hermes-compiler@0.14.0 directly.`,
    );
  }

  return hermesBinaryPath;
}

function ensureTool(command, installHint) {
  try {
    runCommand(command, ['--version']);
  } catch (error) {
    throw new Error(installHint);
  }
}

function ensureStandaloneBuildEnvironment() {
  ensureNodeVersion();
  const jdk = resolveJdkTools();
  const sdkPath = ensureAndroidSdk();
  const gradleWrapperPath = ensureGradleWrapper();
  const hermesBinaryPath = ensureHermesBinary();
  const apksignerPath = ensureApkSigner(sdkPath);

  return {
    apksignerPath,
    gradleWrapperPath,
    hermesBinaryPath,
    jdk,
    sdkPath,
  };
}

function createTempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

function removeDir(targetPath) {
  fs.rmSync(targetPath, { recursive: true, force: true });
}

function assertFileExists(filePath, message) {
  if (!fs.existsSync(filePath)) {
    throw new Error(message);
  }
}

function formatBytes(bytes) {
  if (bytes < 1024) {
    return `${bytes} B`;
  }

  const units = ['KB', 'MB', 'GB'];
  let value = bytes;
  let unitIndex = -1;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  return `${value.toFixed(2)} ${units[unitIndex]}`;
}

function computeSha256(filePath) {
  const hash = crypto.createHash('sha256');
  hash.update(fs.readFileSync(filePath));
  return hash.digest('hex');
}

function listZipEntries(zipPath, jarPath) {
  const result = runCommand(jarPath, ['tf', zipPath]);
  if (result.status !== 0) {
    throw new Error(`Failed to inspect archive entries for ${zipPath}.`);
  }

  return result.stdout
    .split(/\r?\n/)
    .map((entry) => entry.trim())
    .filter((entry) => entry.length > 0);
}

function findNewestApk(apkDirectory) {
  if (!fs.existsSync(apkDirectory)) {
    return null;
  }

  const apkNames = fs
    .readdirSync(apkDirectory)
    .filter((entry) => entry.endsWith('.apk'));

  if (apkNames.length === 0) {
    return null;
  }

  const apkPaths = apkNames.map((name) => path.join(apkDirectory, name));
  apkPaths.sort((left, right) => fs.statSync(right).mtimeMs - fs.statSync(left).mtimeMs);
  return apkPaths[0];
}

function parseBuildToolsVersion(version) {
  const match = version.match(/^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$/);
  if (match === null) {
    return null;
  }

  const prerelease = match[4] ?? null;
  return {
    major: Number.parseInt(match[1], 10),
    minor: Number.parseInt(match[2], 10),
    patch: Number.parseInt(match[3], 10),
    prerelease,
    raw: version,
    stable: prerelease === null,
  };
}

function comparePrerelease(left, right) {
  if (left === right) {
    return 0;
  }
  if (left === null) {
    return 1;
  }
  if (right === null) {
    return -1;
  }

  const rank = (value) => {
    const normalized = value.toLowerCase();
    if (normalized.startsWith('rc')) {
      return 3;
    }
    if (normalized.startsWith('beta')) {
      return 2;
    }
    if (normalized.startsWith('alpha')) {
      return 1;
    }
    return 0;
  };

  const leftRank = rank(left);
  const rightRank = rank(right);
  if (leftRank !== rightRank) {
    return leftRank - rightRank;
  }

  return left.localeCompare(right, undefined, { numeric: true, sensitivity: 'base' });
}

function compareBuildToolsVersions(left, right) {
  if (left.major !== right.major) {
    return left.major - right.major;
  }
  if (left.minor !== right.minor) {
    return left.minor - right.minor;
  }
  if (left.patch !== right.patch) {
    return left.patch - right.patch;
  }
  if (left.stable !== right.stable) {
    return left.stable ? 1 : -1;
  }

  return comparePrerelease(left.prerelease, right.prerelease);
}

function findLatestBuildToolsBinary(sdkPath, binaryName, operations = {}) {
  const fileExists = operations.fileExists ?? fs.existsSync;
  const listDir = operations.listDir ?? fs.readdirSync;
  const buildToolsRoot = path.join(sdkPath, 'build-tools');
  if (!fileExists(buildToolsRoot)) {
    return null;
  }

  const versions = listDir(buildToolsRoot)
    .map((entry) => ({ parsed: parseBuildToolsVersion(entry), raw: entry }))
    .filter((entry) => entry.parsed !== null)
    .sort((left, right) => compareBuildToolsVersions(right.parsed, left.parsed));

  const stableFirst = versions.filter((entry) => entry.parsed.stable);
  const previewFallback = versions.filter((entry) => !entry.parsed.stable);

  for (const entry of [...stableFirst, ...previewFallback]) {
    const candidate = path.join(buildToolsRoot, entry.raw, binaryName);
    if (fileExists(candidate)) {
      return candidate;
    }
  }

  return null;
}

function readApplicationIdFromApk(apkPath, sdkPath) {
  const binaryName = process.platform === 'win32' ? 'aapt.exe' : 'aapt';
  const aaptPath = findLatestBuildToolsBinary(sdkPath, binaryName);
  if (aaptPath === null) {
    throw new Error(
      'Could not find `aapt` inside the configured Android SDK build-tools directory. Install Android build-tools and retry.',
    );
  }

  const result = runCommand(aaptPath, ['dump', 'badging', apkPath]);
  if (result.status !== 0) {
    throw new Error(`Failed to read APK metadata with aapt for ${apkPath}.`);
  }

  const match = result.stdout.match(/package: name='([^']+)'/);
  if (match === null) {
    throw new Error(`Could not determine application ID from ${apkPath}.`);
  }

  return match[1];
}

function ensureApkSigner(sdkPath) {
  const binaryName = process.platform === 'win32' ? 'apksigner.bat' : 'apksigner';
  const apksignerPath = findLatestBuildToolsBinary(sdkPath, binaryName);
  if (apksignerPath === null) {
    throw new Error(
      'Could not find `apksigner` inside the configured Android SDK build-tools directory. Install Android build-tools and retry.',
    );
  }

  return apksignerPath;
}

function verifyDebugSignature(apkPath, apksignerPath) {
  const result = runCommand(apksignerPath, ['verify', '--print-certs', apkPath]);
  if (result.status !== 0) {
    throw new Error(`Failed to inspect APK signing with apksigner for ${apkPath}.`);
  }

  const signingOutput = `${result.stdout}\n${result.stderr}`;
  if (!/Android Debug/i.test(signingOutput)) {
    throw new Error('APK is not signed with the Android debug certificate expected for internal standalone builds.');
  }

  return signingOutput;
}

function runGradleTask(gradleWrapperPath, taskName, androidRoot = MOBILE_ANDROID_ROOT, env = process.env) {
  const command = process.platform === 'win32' ? gradleWrapperPath : './gradlew';
  const result = runCommand(command, [taskName], {
    cwd: androidRoot,
    env,
    stdio: 'inherit',
    shell: process.platform === 'win32',
  });

  if (result.status !== 0) {
    throw new Error(`Gradle task failed: ${taskName}`);
  }
}

module.exports = {
  EXPECTED_APPLICATION_ID,
  EXPECTED_BUNDLE_ENTRY,
  MOBILE_ANDROID_ROOT,
  MOBILE_ROOT,
  assertFileExists,
  buildJdkEnvironment,
  computeSha256,
  createTempDir,
  canonicalizePath,
  detectHermesOsBin,
  ensureApkSigner,
  ensureAndroidSdk,
  ensureGradleWrapper,
  ensureHermesBinary,
  ensureNodeVersion,
  ensureStandaloneBuildEnvironment,
  findNewestApk,
  findLatestBuildToolsBinary,
  formatBytes,
  getHermesBinaryPath,
  getPathEntries,
  listZipEntries,
  parseJavaMajorVersion,
  parseBuildToolsVersion,
  readApplicationIdFromApk,
  removeDir,
  resolveJdkTools,
  resolveAndroidSdkPath,
  runCommand,
  runGradleTask,
  verifyDebugSignature,
};
