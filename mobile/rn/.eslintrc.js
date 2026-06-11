module.exports = {
  root: true,
  extends: '@react-native',
  rules: {
    // Formatting is handled by the opt-in `npm run format` (prettier), not
    // enforced as a lint error — the hand-authored sources use deliberate
    // alignment (e.g. the binding-contract comment boxes) that we keep as-is.
    'prettier/prettier': 'off',

    // Bitwise ops are core to this codebase: decoding the packed cell buffer
    // and ARGB ⇄ channel/CSS conversion (native/cells.ts, theme.ts).
    'no-bitwise': 'off',

    // `void promise` is the intentional "fire-and-forget" marker used across the
    // stores (e.g. `void conn.loadServers()`), so it stays allowed.
    'no-void': 'off',

    // The sources use concise single-line guards (`if (cond) return;`); we don't
    // force braces on them. ESLint still flags real correctness issues.
    curly: 'off',
  },
};
