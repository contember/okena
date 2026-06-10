/**
 * React Native CLI config.
 *
 * `assets` registers the bundled JetBrainsMono ttf files with the native build
 * so `fontFamily: 'JetBrainsMono'` resolves in `<Text>` styles. The Skia
 * terminal renderer loads the same files directly via `useFont(require(...))`
 * (see WorkspaceScreen.tsx), so they are needed both linked and bundled.
 */
module.exports = {
  project: {
    ios: {},
    android: {},
  },
  assets: ['./assets'],
};
