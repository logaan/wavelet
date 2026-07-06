// Headless end-to-end check of the Wavelet VS Code tooling (7.3).
//
// This loads the TextMate grammar (`syntaxes/wavelet.tmLanguage.json`) with
// `vscode-textmate` + `vscode-oniguruma` — the *same* tokenizer engine VS Code
// itself uses — and asserts that representative Wavelet snippets tokenize to the
// scopes the language config and themes rely on. It also validates that
// `package.json` and `language-configuration.json` are well-formed and mutually
// consistent. Run it with `npm run check` from `tooling/vscode/`.
//
// Keeping this in step with `src/lexer.rs` and `docs/src/prism/wavelet.js` is
// what "the grammar actually works in-editor" means for CI purposes; the
// remaining GUI-only checks (bracket matching, auto-closing, LSP wiring) are
// listed as a manual smoke checklist in README.md.

import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);
// Both packages ship CommonJS; require() gives the correct namespace shape
// under Node ESM (a bare `import *` puts the exports under `.default`).
const vsctm = require("vscode-textmate");
const oniguruma = require("vscode-oniguruma");
const here = new URL(".", import.meta.url);
const grammarPath = fileURLToPath(new URL("../syntaxes/wavelet.tmLanguage.json", here));
const pkgPath = fileURLToPath(new URL("../package.json", here));
const langCfgPath = fileURLToPath(new URL("../language-configuration.json", here));

let failures = 0;
const fail = (msg) => {
  failures += 1;
  console.error(`  FAIL  ${msg}`);
};
const pass = (msg) => console.log(`  ok    ${msg}`);

async function loadGrammar() {
  const wasm = await readFile(require.resolve("vscode-oniguruma/release/onig.wasm"));
  await oniguruma.loadWASM(wasm);
  const onigLib = Promise.resolve({
    createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
    createOnigString: (s) => new oniguruma.OnigString(s),
  });
  const registry = new vsctm.Registry({
    onigLib,
    loadGrammar: async (scopeName) => {
      if (scopeName !== "source.wavelet") return null;
      const data = await readFile(grammarPath, "utf8");
      return vsctm.parseRawGrammar(data, grammarPath);
    },
  });
  const grammar = await registry.loadGrammar("source.wavelet");
  if (!grammar) throw new Error("grammar source.wavelet failed to load");
  return grammar;
}

// (line, tokenSubstring, expectedScope) — assert the token covering the first
// occurrence of `sub` in `line` carries `scope`.
const CASES = [
  ["#!/usr/bin/env wavelet", "#!", "comment.line.number-sign.shebang.wavelet"],
  ["add(1 2) // sum", "//", "comment.line.double-slash.wavelet"],
  ['str-cat("hi")', "hi", "string.quoted.double.wavelet"],
  ["'a'", "'a'", "string.quoted.single.wavelet"],
  ["mul(x 42)", "42", "constant.numeric.wavelet"],
  ["div(1.0 -inf)", "-inf", "constant.numeric.wavelet"],
  ["eq(b true)", "true", "constant.language.boolean.wavelet"],
  ["ok(some(x))", "some", "constant.language.wavelet"],
  ["If cond a b", "If", "keyword.control.macro.wavelet"],
  ["upper(phrase)", "upper", "entity.name.function.wavelet"],
  // A qualified *call* (`sh/shout(`) highlights as one function name; the
  // namespace scope is for a free-standing qualified *value* (no `(`).
  ["map(sh/shout xs)", "sh", "entity.name.namespace.wavelet"],
  ["sh/shout(x)", "sh/shout", "entity.name.function.wavelet"],
  ['make({phrase: "x"})', "phrase", "variable.other.property.wavelet"],
];

function tokenScopesAt(grammar, line, sub) {
  const idx = line.indexOf(sub);
  if (idx < 0) throw new Error(`substring ${JSON.stringify(sub)} not in ${JSON.stringify(line)}`);
  const { tokens } = grammar.tokenizeLine(line, vsctm.INITIAL);
  // The token covering the start of the substring.
  const tok = tokens.find((t) => t.startIndex <= idx && idx < t.endIndex);
  return tok ? tok.scopes : [];
}

async function checkGrammar(grammar) {
  console.log("grammar scope assertions:");
  for (const [line, sub, scope] of CASES) {
    const scopes = tokenScopesAt(grammar, line, sub);
    if (scopes.includes(scope)) pass(`${JSON.stringify(sub)} -> ${scope}`);
    else fail(`${JSON.stringify(sub)} in ${JSON.stringify(line)} expected ${scope}, got [${scopes.join(", ")}]`);
  }
}

async function checkManifest() {
  console.log("manifest consistency:");
  const pkg = JSON.parse(await readFile(pkgPath, "utf8"));
  JSON.parse(await readFile(langCfgPath, "utf8")); // valid JSON or throws
  const lang = pkg.contributes?.languages?.[0];
  const gram = pkg.contributes?.grammars?.[0];
  if (lang?.id === "wavelet") pass("language id is `wavelet`"); else fail("language id");
  if (lang?.extensions?.includes(".wlt")) pass("`.wlt` is a registered extension"); else fail(".wlt extension");
  if (lang?.configuration === "./language-configuration.json") pass("language-configuration path"); else fail("language-configuration path");
  if (gram?.scopeName === "source.wavelet") pass("grammar scopeName matches"); else fail("grammar scopeName");
  if (gram?.path === "./syntaxes/wavelet.tmLanguage.json") pass("grammar path"); else fail("grammar path");
  const grammarRaw = JSON.parse(await readFile(grammarPath, "utf8"));
  if (grammarRaw.scopeName === gram?.scopeName) pass("grammar file scopeName agrees with manifest"); else fail("grammar/manifest scopeName mismatch");
  if (grammarRaw.fileTypes?.includes("wlt")) pass("grammar fileTypes include wlt"); else fail("grammar fileTypes");
}

const grammar = await loadGrammar();
await checkManifest();
await checkGrammar(grammar);

if (failures > 0) {
  console.error(`\n${failures} check(s) FAILED`);
  process.exit(1);
}
console.log("\nAll VS Code tooling checks passed.");
