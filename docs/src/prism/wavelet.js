export const waveletGrammar = {

  'shebang': {
    pattern: /^#!.*/,
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

  'macro': {
    pattern: /\b(?:[A-Za-z][A-Za-z0-9]*[-\/])*[A-Z][A-Za-z0-9]*[a-z][A-Za-z0-9]*(?:[-\/][A-Za-z][A-Za-z0-9]*)*\b/,
    alias: 'keyword',
  },
  'keyword': /\b(?:some|none|ok|err)\b/,

  'placeholder': {
    pattern: /(^|[^\w-])_(?![\w-])/,
    lookbehind: true,
    alias: 'keyword',
  },
  'number': /-?\b(?:inf|nan|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b/,

  'function': {
    pattern: /%?[A-Za-z][\w-]*(?:\/%?[\w-]+)*(?=\()/,
    greedy: true,
  },

  'property': /%?[A-Za-z][\w-]*(?=\s*:)/,

  'punctuation': /[(){}\[\],:/.]/,
};

export function registerWavelet(Prism) {
  if (Prism && Prism.languages && !Prism.languages.wavelet) {
    Prism.languages.wavelet = waveletGrammar;
  }
}
