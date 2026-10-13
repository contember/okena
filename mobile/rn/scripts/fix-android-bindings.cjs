const fs = require('node:fs');
const path = require('node:path');

const android = path.resolve(__dirname, '../modules/okena-mobile-ffi/android');
const cmakePath = path.join(android, 'CMakeLists.txt');
const cmake = fs.readFileSync(cmakePath, 'utf8');
const original = `node -p "require.resolve('uniffi-bindgen-react-native/package.json')"`;
const replacement = `node -e "const path = require('path'); process.stdout.write(path.resolve(path.dirname(require.resolve('uniffi-bindgen-react-native')), '../../..', 'package.json'))"`;

// ubrn 0.31 resolves a package.json subpath that its own exports map hides.
if (!cmake.includes(original) && !cmake.includes(replacement)) {
  throw new Error(
    'ubrn CMake template changed; review the package resolution fix.',
  );
}
fs.writeFileSync(cmakePath, cmake.replace(original, replacement));
fs.writeFileSync(
  path.join(android, 'src/main/AndroidManifestNew.xml'),
  '<manifest />\n',
);
