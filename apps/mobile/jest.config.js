module.exports = {
  preset: 'react-native',
  moduleNameMapper: {
    '^@anchor/protocol$': '<rootDir>/../../packages/protocol/src/index.ts',
  },
  transformIgnorePatterns: [
    'node_modules/(?!(?:.pnpm/)?((jest-)?react-native|@react-native(?:-community)?|react-native-safe-area-context))',
  ],
};
