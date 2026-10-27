module.exports = {
  preset: '@react-native/jest-preset',
  globalSetup: './jest.globalSetup.js',
  // The preset's pattern plus the document picker, which ships ES modules.
  transformIgnorePatterns: [
    'node_modules/(?!((jest-)?react-native|@react-native(-community|-documents)?)/)',
  ],
  // Added to the preset's setup files: mocks the picker's native module.
  setupFiles: ['<rootDir>/node_modules/@react-native-documents/picker/jest/build/jest/setup.js'],
};
