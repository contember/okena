const {getDefaultConfig, mergeConfig} = require('@react-native/metro-config');
const path = require('path');

/**
 * Metro configuration
 * https://reactnative.dev/docs/metro
 *
 * @type {import('@react-native/metro-config').MetroConfig}
 */

// ubrn (uniffi-bindgen-react-native) 0.31 generates bindings that import the
// TypeScript runtime as `@ubjs/core` (its new published identity). That runtime
// is the *same bytes* already shipped inside `uniffi-bindgen-react-native`
// (typescript/dist), so we alias `@ubjs/core` to it rather than installing a
// second, potentially version-skewed copy. See node_modules/uniffi-bindgen-react-native/README.md.
const ubrnRuntime = path.resolve(
  __dirname,
  'node_modules/uniffi-bindgen-react-native/typescript/dist/cjs/index.js',
);

const config = {
  resolver: {
    resolveRequest: (context, moduleName, platform) => {
      if (moduleName === '@ubjs/core') {
        return {type: 'sourceFile', filePath: ubrnRuntime};
      }
      return context.resolveRequest(context, moduleName, platform);
    },
  },
};

module.exports = mergeConfig(getDefaultConfig(__dirname), config);
