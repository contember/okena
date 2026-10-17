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
let fixed = cmake.replace(original, replacement);

// Native terminal view: compile the JNI shim over crates/okena-mobile-ffi's C
// render ABI into this same .so, so Kotlin reaches the one Rust copy (and its
// ConnectionManager singleton) the JSI module uses. Linking the Rust .a into a
// second .so would create a second, empty singleton.
const ffiCrate = path.resolve(__dirname, '../../../crates/okena-mobile-ffi');
const shim = path.join(ffiCrate, 'jni/okena_render_jni.cpp');
const sourcesAnchor = '    cpp-adapter.cpp\n)';
const renderMarker = '# okena-render (fix-android-bindings.cjs)';
if (!fixed.includes(renderMarker)) {
  if (!fixed.includes(sourcesAnchor)) {
    throw new Error(
      'ubrn CMake template changed; review the okena-render shim injection.',
    );
  }
  fixed = fixed.replace(
    sourcesAnchor,
    `    cpp-adapter.cpp\n    ${shim}\n)`,
  );
  fixed += `
${renderMarker}
target_include_directories(okena-mobile-ffi PRIVATE ${path.join(ffiCrate, 'include')})
# Keep every okena_rv_* entry point (and export it from the .so) even if the
# shim does not call it yet; archive members are otherwise dropped.
target_link_options(okena-mobile-ffi PRIVATE "-Wl,--undefined-glob=okena_rv_*")
`;
}
fs.writeFileSync(cmakePath, fixed);
fs.writeFileSync(
  path.join(android, 'src/main/AndroidManifestNew.xml'),
  '<manifest />\n',
);
