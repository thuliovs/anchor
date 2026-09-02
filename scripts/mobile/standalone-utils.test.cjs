const test = require('node:test');
const assert = require('node:assert/strict');

const {
  buildJdkEnvironment,
  detectHermesOsBin,
  findLatestBuildToolsBinary,
  parseJavaMajorVersion,
  resolveJdkTools,
  resolveAndroidSdkPath,
} = require('./standalone-utils.cjs');

function createJdkResolverFixture({
  env = {},
  pathEntries = [],
  existingPaths = [],
  commandResults = {},
  realPaths = {},
  platform = 'linux',
} = {}) {
  const existing = new Set(existingPaths);
  const calls = [];

  const result = resolveJdkTools({
    env,
    platform,
    pathEntries,
    fileExists(filePath) {
      return existing.has(filePath);
    },
    execFile(command, args) {
      calls.push(command);
      const output = commandResults[`${command} ${args.join(' ')}`] ?? commandResults[command];
      if (output instanceof Error) {
        throw output;
      }
      if (output === undefined) {
        throw new Error(`unexpected command: ${command}`);
      }

      return { stdout: output.stdout ?? '', stderr: output.stderr ?? '', status: 0 };
    },
    realpath(targetPath) {
      return realPaths[targetPath] ?? targetPath;
    },
  });

  return { calls, result };
}

function createBuildToolsFixture(entries, availableFiles, binaryName, platform = 'linux') {
  return findLatestBuildToolsBinary('/sdk', binaryName, {
    platform,
    listDir(dirPath) {
      assert.equal(dirPath, '/sdk/build-tools');
      return entries;
    },
    fileExists(filePath) {
      return filePath === '/sdk/build-tools' || availableFiles.includes(filePath);
    },
  });
}

test('parseJavaMajorVersion extracts the major version from JDK 21 output', () => {
  assert.equal(parseJavaMajorVersion('openjdk version "21.0.8" 2026-07-15'), 21);
  assert.equal(parseJavaMajorVersion('javac 21.0.8'), 21);
});

test('parseJavaMajorVersion returns null when no version can be parsed', () => {
  assert.equal(parseJavaMajorVersion('java version "foobar"'), null);
});

test('detectHermesOsBin maps supported host platforms', () => {
  assert.equal(detectHermesOsBin('linux', 'x64'), 'linux64-bin');
  assert.equal(detectHermesOsBin('darwin', 'arm64'), 'osx-bin');
  assert.equal(detectHermesOsBin('win32', 'x64'), 'win64-bin');
});

test('detectHermesOsBin rejects unsupported platforms', () => {
  assert.throws(() => detectHermesOsBin('sunos', 'x64'), /Unsupported platform/);
});

test('resolveAndroidSdkPath prefers explicit environment variables', () => {
  assert.equal(
    resolveAndroidSdkPath({ ANDROID_HOME: '/sdk/home', ANDROID_SDK_ROOT: '/sdk/root' }, null),
    '/sdk/home',
  );
  assert.equal(resolveAndroidSdkPath({ ANDROID_SDK_ROOT: '/sdk/root' }, null), '/sdk/root');
});

test('resolveAndroidSdkPath falls back to local.properties sdk.dir', () => {
  assert.equal(resolveAndroidSdkPath({}, 'sdk.dir=/opt/android-sdk'), '/opt/android-sdk');
});

test('resolveJdkTools prefers JAVA_HOME even when PATH points to another JDK', () => {
  const javaHome = '/jdks/jdk-21';
  const { result, calls } = createJdkResolverFixture({
    env: { JAVA_HOME: javaHome },
    pathEntries: ['/jdks/jdk-26/bin'],
    existingPaths: [
      '/jdks/jdk-21/bin/java',
      '/jdks/jdk-21/bin/javac',
      '/jdks/jdk-21/bin/jar',
      '/jdks/jdk-26/bin/java',
      '/jdks/jdk-26/bin/javac',
      '/jdks/jdk-26/bin/jar',
    ],
    commandResults: {
      '/jdks/jdk-21/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /jdks/jdk-21\nopenjdk version "21.0.8"',
      },
      '/jdks/jdk-21/bin/javac -version': { stderr: 'javac 21.0.8' },
      '/jdks/jdk-21/bin/jar --version': { stdout: 'jar 21.0.8' },
    },
  });

  assert.equal(result.javaHome, javaHome);
  assert.equal(result.jarVersion, 'jar 21.0.8');
  assert.deepEqual(calls, [
    '/jdks/jdk-21/bin/java',
    '/jdks/jdk-21/bin/javac',
    '/jdks/jdk-21/bin/jar',
  ]);
});

test('resolveJdkTools accepts JAVA_HOME symlink when java.home reports the real JDK path', () => {
  const { result } = createJdkResolverFixture({
    env: { JAVA_HOME: '/links/jdk-21' },
    existingPaths: [
      '/links/jdk-21/bin/java',
      '/links/jdk-21/bin/javac',
      '/links/jdk-21/bin/jar',
    ],
    realPaths: {
      '/links/jdk-21': '/real/jdk-21',
      '/real/jdk-21': '/real/jdk-21',
    },
    commandResults: {
      '/links/jdk-21/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /real/jdk-21/\nopenjdk version "21.0.12"',
      },
      '/links/jdk-21/bin/javac -version': { stderr: 'javac 21.0.12' },
      '/links/jdk-21/bin/jar --version': { stdout: 'jar 21.0.12' },
    },
  });

  assert.equal(result.javaHome, '/real/jdk-21');
});

test('resolveJdkTools returns the canonical javaHome instead of the symlink text', () => {
  const { result } = createJdkResolverFixture({
    env: { JAVA_HOME: '/links/jdk-21/' },
    existingPaths: [
      '/links/jdk-21/bin/java',
      '/links/jdk-21/bin/javac',
      '/links/jdk-21/bin/jar',
    ],
    realPaths: {
      '/links/jdk-21/': '/real/jdk-21',
      '/links/jdk-21': '/real/jdk-21',
      '/real/jdk-21/': '/real/jdk-21',
      '/real/jdk-21': '/real/jdk-21',
    },
    commandResults: {
      '/links/jdk-21/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /real/jdk-21\nopenjdk version "21.0.12"',
      },
      '/links/jdk-21/bin/javac -version': { stderr: 'javac 21.0.12' },
      '/links/jdk-21/bin/jar --version': { stdout: 'jar 21.0.12' },
    },
  });

  assert.equal(result.javaHome, '/real/jdk-21');
});

test('resolveJdkTools rejects incompatible JAVA_HOME even when PATH has JDK 21', () => {
  assert.throws(
    () =>
      createJdkResolverFixture({
        env: { JAVA_HOME: '/jdks/jdk-26' },
        pathEntries: ['/jdks/jdk-21/bin'],
        existingPaths: [
          '/jdks/jdk-26/bin/java',
          '/jdks/jdk-26/bin/javac',
          '/jdks/jdk-26/bin/jar',
          '/jdks/jdk-21/bin/java',
          '/jdks/jdk-21/bin/javac',
          '/jdks/jdk-21/bin/jar',
        ],
        commandResults: {
          '/jdks/jdk-26/bin/java -XshowSettings:properties -version': {
            stderr: 'Property settings:\n    java.home = /jdks/jdk-26\nopenjdk version "26.0.2"',
          },
          '/jdks/jdk-26/bin/javac -version': { stderr: 'javac 26.0.2' },
          '/jdks/jdk-26/bin/jar --version': { stdout: 'jar 26.0.2' },
        },
      }),
    /JDK 21 is required[\s\S]*JAVA_HOME=.*jdk-26[\s\S]*26\.0\.2/,
  );
});

test('resolveJdkTools uses PATH consistently when JAVA_HOME is absent', () => {
  const { result, calls } = createJdkResolverFixture({
    pathEntries: ['/usr/bin', '/jdks/jdk-21/bin', '/jdks/jdk-26/bin'],
    existingPaths: [
      '/usr/bin/java',
      '/usr/bin/javac',
      '/usr/bin/jar',
      '/jdks/jdk-21/bin/java',
      '/jdks/jdk-21/bin/javac',
      '/jdks/jdk-21/bin/jar',
      '/jdks/jdk-26/bin/java',
      '/jdks/jdk-26/bin/javac',
      '/jdks/jdk-26/bin/jar',
    ],
    commandResults: {
      '/usr/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /jdks/jdk-21\nopenjdk version "21.0.12"',
      },
      '/jdks/jdk-21/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /jdks/jdk-21\nopenjdk version "21.0.12"',
      },
      '/jdks/jdk-21/bin/javac -version': { stderr: 'javac 21.0.12' },
      '/jdks/jdk-21/bin/jar --version': { stdout: 'jar 21.0.12' },
    },
  });

  assert.equal(result.javaHome, '/jdks/jdk-21');
  assert.equal(result.javaPath, '/jdks/jdk-21/bin/java');
  assert.equal(result.javacPath, '/jdks/jdk-21/bin/javac');
  assert.equal(result.jarPath, '/jdks/jdk-21/bin/jar');
  assert.deepEqual(calls, [
    '/usr/bin/java',
    '/jdks/jdk-21/bin/java',
    '/jdks/jdk-21/bin/javac',
    '/jdks/jdk-21/bin/jar',
  ]);
});

test('resolveJdkTools uses canonical java.home instead of /usr when PATH contains shims', () => {
  const { result } = createJdkResolverFixture({
    pathEntries: ['/usr/bin'],
    existingPaths: [
      '/usr/bin/java',
      '/usr/bin/javac',
      '/usr/bin/jar',
      '/real/jdk-21/bin/java',
      '/real/jdk-21/bin/javac',
      '/real/jdk-21/bin/jar',
    ],
    commandResults: {
      '/usr/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /real/jdk-21\nopenjdk version "21.0.12"',
      },
      '/real/jdk-21/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /real/jdk-21\nopenjdk version "21.0.12"',
      },
      '/real/jdk-21/bin/javac -version': { stderr: 'javac 21.0.12' },
      '/real/jdk-21/bin/jar --version': { stdout: 'jar 21.0.12' },
    },
  });

  assert.equal(result.javaHome, '/real/jdk-21');
  assert.notEqual(result.javaHome, '/usr');
});

test('resolveJdkTools accepts equivalent paths with trailing separators', () => {
  const { result } = createJdkResolverFixture({
    env: { JAVA_HOME: '/jdks/jdk-21/' },
    existingPaths: [
      '/jdks/jdk-21/bin/java',
      '/jdks/jdk-21/bin/javac',
      '/jdks/jdk-21/bin/jar',
    ],
    realPaths: {
      '/jdks/jdk-21/': '/jdks/jdk-21',
      '/jdks/jdk-21': '/jdks/jdk-21',
    },
    commandResults: {
      '/jdks/jdk-21/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /jdks/jdk-21/\nopenjdk version "21.0.12"',
      },
      '/jdks/jdk-21/bin/javac -version': { stderr: 'javac 21.0.12' },
      '/jdks/jdk-21/bin/jar --version': { stdout: 'jar 21.0.12' },
    },
  });

  assert.equal(result.javaHome, '/jdks/jdk-21');
});

test('resolveJdkTools fails clearly when no valid JDK 21 exists', () => {
  assert.throws(
    () =>
      createJdkResolverFixture({
        pathEntries: ['/jdks/jre/bin'],
        existingPaths: ['/jdks/jre/bin/java'],
        commandResults: {
          '/jdks/jre/bin/java -XshowSettings:properties -version': {
            stderr: 'Property settings:\n    java.home = /jdks/jre\nopenjdk version "17.0.9"',
          },
        },
      }),
    /JDK 21 is required[\s\S]*Install JDK 21 or set JAVA_HOME/,
  );
});

test('resolveJdkTools rejects mismatched jar version', () => {
  assert.throws(
    () =>
      createJdkResolverFixture({
        env: { JAVA_HOME: '/jdks/jdk-21' },
        existingPaths: [
          '/jdks/jdk-21/bin/java',
          '/jdks/jdk-21/bin/javac',
          '/jdks/jdk-21/bin/jar',
        ],
        commandResults: {
          '/jdks/jdk-21/bin/java -XshowSettings:properties -version': {
            stderr: 'Property settings:\n    java.home = /jdks/jdk-21\nopenjdk version "21.0.12"',
          },
          '/jdks/jdk-21/bin/javac -version': { stderr: 'javac 21.0.12' },
          '/jdks/jdk-21/bin/jar --version': { stdout: 'jar 26.0.2' },
        },
      }),
    /jar `jar 26\.0\.2`/,
  );
});

test('resolveJdkTools accepts java, javac and jar from the same JDK 21', () => {
  const { result } = createJdkResolverFixture({
    env: { JAVA_HOME: '/jdks/jdk-21' },
    existingPaths: [
      '/jdks/jdk-21/bin/java',
      '/jdks/jdk-21/bin/javac',
      '/jdks/jdk-21/bin/jar',
    ],
    commandResults: {
      '/jdks/jdk-21/bin/java -XshowSettings:properties -version': {
        stderr: 'Property settings:\n    java.home = /jdks/jdk-21\nopenjdk version "21.0.12"',
      },
      '/jdks/jdk-21/bin/javac -version': { stderr: 'javac 21.0.12' },
      '/jdks/jdk-21/bin/jar --version': { stdout: 'jar 21.0.12' },
    },
  });

  assert.equal(result.javaHome, '/jdks/jdk-21');
  assert.equal(result.javaVersion.includes('21.0.12'), true);
  assert.equal(result.javacVersion, 'javac 21.0.12');
  assert.equal(result.jarVersion, 'jar 21.0.12');
});

test('resolveJdkTools resolves java, javac and jar inside JAVA_HOME on Windows', () => {
  const javaHome = 'C:\\Java\\jdk-21';
  const { result } = createJdkResolverFixture({
    env: { JAVA_HOME: javaHome },
    platform: 'win32',
    existingPaths: [
      'C:\\Java\\jdk-21\\bin\\java.exe',
      'C:\\Java\\jdk-21\\bin\\javac.exe',
      'C:\\Java\\jdk-21\\bin\\jar.exe',
    ],
    commandResults: {
      'C:\\Java\\jdk-21\\bin\\java.exe -XshowSettings:properties -version': {
        stderr: 'Property settings:\r\n    java.home = C:\\Java\\jdk-21\r\nopenjdk version "21.0.8"',
      },
      'C:\\Java\\jdk-21\\bin\\javac.exe -version': { stderr: 'javac 21.0.8' },
      'C:\\Java\\jdk-21\\bin\\jar.exe --version': { stdout: 'jar 21.0.8' },
    },
  });

  assert.equal(result.javaHome, 'c:\\java\\jdk-21');
  assert.equal(result.javaPath, 'C:\\Java\\jdk-21\\bin\\java.exe');
  assert.equal(result.javacPath, 'C:\\Java\\jdk-21\\bin\\javac.exe');
  assert.equal(result.jarPath, 'C:\\Java\\jdk-21\\bin\\jar.exe');
});

test('resolveJdkTools accepts equivalent Windows paths with different casing and separators', () => {
  const { result } = createJdkResolverFixture({
    env: { JAVA_HOME: 'c:/java/JDK-21\\' },
    platform: 'win32',
    existingPaths: [
      'c:\\java\\JDK-21\\bin\\java.exe',
      'c:\\java\\JDK-21\\bin\\javac.exe',
      'c:\\java\\JDK-21\\bin\\jar.exe',
    ],
    realPaths: {
      'c:\\java\\JDK-21\\': 'C:\\Java\\jdk-21',
      'c:\\java\\JDK-21': 'C:\\Java\\jdk-21',
      'C:\\Java\\jdk-21': 'C:\\Java\\jdk-21',
    },
    commandResults: {
      'c:\\java\\JDK-21\\bin\\java.exe -XshowSettings:properties -version': {
        stderr: 'Property settings:\r\n    java.home = C:/Java/jdk-21/\r\nopenjdk version "21.0.8"',
      },
      'c:\\java\\JDK-21\\bin\\javac.exe -version': { stderr: 'javac 21.0.8' },
      'c:\\java\\JDK-21\\bin\\jar.exe --version': { stdout: 'jar 21.0.8' },
    },
  });

  assert.equal(result.javaHome, 'c:\\java\\jdk-21');
});

test('resolveJdkTools rejects JAVA_HOME when canonicalized path is a different installation', () => {
  assert.throws(
    () =>
      createJdkResolverFixture({
        env: { JAVA_HOME: '/links/jdk-21' },
        existingPaths: [
          '/links/jdk-21/bin/java',
          '/links/jdk-21/bin/javac',
          '/links/jdk-21/bin/jar',
        ],
        realPaths: {
          '/links/jdk-21': '/real/jdk-21-a',
          '/real/jdk-21-b': '/real/jdk-21-b',
        },
        commandResults: {
          '/links/jdk-21/bin/java -XshowSettings:properties -version': {
            stderr: 'Property settings:\n    java.home = /real/jdk-21-b\nopenjdk version "21.0.12"',
          },
          '/links/jdk-21/bin/javac -version': { stderr: 'javac 21.0.12' },
          '/links/jdk-21/bin/jar --version': { stdout: 'jar 21.0.12' },
        },
      }),
    /JDK resolution mismatch[\s\S]*real\/jdk-21-a[\s\S]*real\/jdk-21-b/,
  );
});

test('buildJdkEnvironment passes the canonical JAVA_HOME to Gradle', () => {
  const env = buildJdkEnvironment(
    {
      javaHome: '/real/jdk-21',
      javaPath: '/real/jdk-21/bin/java',
      javacPath: '/real/jdk-21/bin/javac',
      jarPath: '/real/jdk-21/bin/jar',
    },
    { PATH: '/usr/bin:/bin' },
    'linux',
  );

  assert.equal(env.JAVA_HOME, '/real/jdk-21');
  assert.equal(env.PATH.startsWith('/real/jdk-21/bin:'), true);
});

test('findLatestBuildToolsBinary prefers highest stable numeric version', () => {
  const result = createBuildToolsFixture(
    ['9.0.0', '35.0.0', '36.0.0'],
    ['/sdk/build-tools/9.0.0/aapt', '/sdk/build-tools/35.0.0/aapt', '/sdk/build-tools/36.0.0/aapt'],
    'aapt',
  );

  assert.equal(result, '/sdk/build-tools/36.0.0/aapt');
});

test('findLatestBuildToolsBinary prefers stable release over rc with same numeric version', () => {
  const result = createBuildToolsFixture(
    ['36.1.0-rc1', '36.1.0'],
    ['/sdk/build-tools/36.1.0-rc1/apksigner', '/sdk/build-tools/36.1.0/apksigner'],
    'apksigner',
  );

  assert.equal(result, '/sdk/build-tools/36.1.0/apksigner');
});

test('findLatestBuildToolsBinary skips newest version when requested binary is absent', () => {
  const result = createBuildToolsFixture(
    ['35.0.0', '36.0.0'],
    ['/sdk/build-tools/35.0.0/aapt'],
    'aapt',
  );

  assert.equal(result, '/sdk/build-tools/35.0.0/aapt');
});

test('findLatestBuildToolsBinary returns null when sdk has no build-tools', () => {
  const result = findLatestBuildToolsBinary('/sdk', 'aapt', {
    fileExists(filePath) {
      return filePath === '/sdk/build-tools' ? false : false;
    },
    listDir() {
      throw new Error('should not list missing build-tools');
    },
  });

  assert.equal(result, null);
});

test('findLatestBuildToolsBinary falls back to preview when no stable version is available', () => {
  const result = createBuildToolsFixture(
    ['36.1.0-beta1', '36.1.0-rc1'],
    ['/sdk/build-tools/36.1.0-beta1/aapt', '/sdk/build-tools/36.1.0-rc1/aapt'],
    'aapt',
  );

  assert.equal(result, '/sdk/build-tools/36.1.0-rc1/aapt');
});
