import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const require = createRequire(import.meta.url);

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

  ["map(sh/shout xs)", "sh", "source.wavelet"],
  ["lib/Identity 42", "lib/Identity", "keyword.control.macro.wavelet"],
  ["lib-Identity 42", "lib-Identity", "keyword.control.macro.wavelet"],
  ["nested/api/get(1)", "nested/api/get", "entity.name.function.wavelet"],
  ["sh/shout(x)", "sh/shout", "entity.name.function.wavelet"],
  ['make({phrase: "x"})', "phrase", "variable.other.property.wavelet"],
];

function tokenScopesAt(grammar, line, sub) {
  const idx = line.indexOf(sub);
  if (idx < 0) throw new Error(`substring ${JSON.stringify(sub)} not in ${JSON.stringify(line)}`);
  const { tokens } = grammar.tokenizeLine(line, vsctm.INITIAL);

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
  JSON.parse(await readFile(langCfgPath, "utf8"));
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
