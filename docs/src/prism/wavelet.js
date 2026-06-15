// Prism grammar for Wavelet (WAVE text + reader sugar).
//
// Shared by two surfaces so highlighting can never drift between them:
//   - static ```wavelet code blocks (src/theme/prism-include-languages.js)
//   - the live <Playground> editor (src/components/Playground)
//
// Token classes mirror the lexer (src/lexer.rs):
//   - `//` line comments and `///` doc comments
//   - "..." strings and '.' chars, both with \u{...}/\n/... escapes
//   - int / float / inf / nan numbers
//   - true / false booleans; some / none / ok / err WAVE constructors
//   - TitleCase macro heads (If, Def, Fn, DefMacro, ...)
//   - call heads: a name attached (no space) to ( [ or { — the attachment rule
//   - alias/name qualified references
//   - record keys (name:)

export const waveletGrammar = {
  // `///` doc comments attach to the next form; style them like other comments
  // but keep the class distinct in case the theme wants to.
  'doc-comment': {
    pattern: /\/\/\/(?!\/).*/,
    alias: 'comment',
    greedy: true,
  },
  'comment': {
    pattern: /\/\/.*/,
    greedy: true,
  },
  'string': {
    pattern: /"(?:\\[\s\S]|[^"\\])*"/,
    greedy: true,
  },
  'char': {
    pattern: /'(?:\\(?:u\{[0-9a-fA-F]+\}|[\s\S])|[^\\'])'/,
    greedy: true,
    alias: 'string',
  },
  'boolean': /\b(?:true|false)\b/,
  // TitleCase: leading capital, at least one lowercase, and no hyphen — exactly
  // the lexer's `is_title`. Reader sugar for a macro call, so style as keyword.
  'macro': {
    pattern: /\b[A-Z][A-Za-z0-9]*[a-z][A-Za-z0-9]*\b/,
    alias: 'keyword',
  },
  'keyword': /\b(?:some|none|ok|err)\b/,
  'number': /-?\b(?:inf|nan|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b/,
  // Attachment rule: a (possibly %-escaped, possibly qualified) name immediately
  // followed by ( is a call head. Only `(` attaches now — a name before `[`/`{`
  // is an ordinary name/reference, not a call head.
  'function': {
    pattern: /%?[A-Za-z][\w-]*(?:\/%?[\w-]+)?(?=\()/,
    greedy: true,
  },
  // The alias side of a free-standing qualified reference (`kv/get` as a value).
  'namespace': /%?[A-Za-z][\w-]*(?=\/)/,
  // Record keys: `name:`.
  'property': /%?[A-Za-z][\w-]*(?=\s*:)/,
  'punctuation': /[(){}\[\],:/]/,
};

// Register the grammar on a Prism instance (the global one used for static code
// blocks, or prism-react-renderer's instance used by the Playground).
export function registerWavelet(Prism) {
  if (Prism && Prism.languages && !Prism.languages.wavelet) {
    Prism.languages.wavelet = waveletGrammar;
  }
}
