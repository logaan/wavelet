import {compiledCases} from './compiled-cases.mjs';
import {importComponentBindings as importModule} from '../scripts/node-component-loader.mjs';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
import {instantiateComponent} from '../src/runtime/componentLoader.mjs';
import {createCompilerRuntime, formatError} from '../src/runtime/compilerRuntime.mjs';


test('transpiles and executes a real component with explicit instantiation', async () => {
  const bytes = await readFile(new URL('../../tests/fixtures/add.wasm', import.meta.url));
  const component = await instantiateComponent(bytes, {}, importModule);
  assert.equal(component.add(20, 22), 42);
});

function compilerHarness(source = {tag: 'component', val: Uint8Array.of(2)}) {
  const calls = [];
  const instantiate = async (bytes, imports) => {
    calls.push({bytes, imports});
    switch (bytes[0]) {
      case 1: return {'wavelet:compiler/compiler@0.1.0': {compile(request, bindings) {
        if (!bindings.length) return {tag: 'needs-macros', val: [{key: 'macro-key', source}]};
        assert.deepEqual(bindings[0].library.manifest(), [['answer', 0]]);
        assert.equal(bindings[0].library.expand('answer', 'tree'), 'expanded:tree');
        return {tag: 'complete', val: {artifacts: [{name: 'snippet', bytes: Uint8Array.of(3)}], entry: 'snippet', invocation: {interfaceName: 'example:snippet/custom@1.2.3', functionName: 'get-answer'}}};
      }}};
      case 2: return {'wavelet:meta/macros@0.1.0': {manifest: () => [['answer', 0]], expand: (_, tree) => `expanded:${tree}`}};
      case 3: return {'example:snippet/custom@1.2.3': {getAnswer: () => '42'}};
      default: throw new Error('Unexpected component');
    }
  };
  return {calls, instantiate};
}

test('prepares asynchronous macro libraries before synchronous compiler callbacks', async () => {
  const harness = compilerHarness();
  const runtime = await createCompilerRuntime(Uint8Array.of(1), harness);
  assert.deepEqual(await runtime.evaluate({source: 'answer()'}), {ok: true, value: '42', output: '', error: ''});
  assert.deepEqual(harness.calls.map(({bytes}) => bytes[0]), [1, 2, 3]);
  assert.equal(typeof harness.calls[0].imports['wavelet:compiler/macro-host'].Library, 'function');
});

test('imported macros require explicit supplied artifacts', async () => {
  const harness = compilerHarness({tag: 'imported', val: {packageId: 'example:macros', sourcePath: 'macros.wasm'}});
  const runtime = await createCompilerRuntime(Uint8Array.of(1), harness);
  await assert.rejects(runtime.evaluate({source: 'answer()'}), /Supply exactly one macro component/);
  const outcome = await runtime.evaluate({source: 'answer()', macroComponents: [{name: 'macros.wasm', contents: Uint8Array.of(2)}]});
  assert.equal(outcome.value, '42');
});

test('compiler diagnostics preserve source and phase', () => {
  assert.equal(formatError([{source: 'demo.wlt', phase: 'check', message: 'Unknown binding.'}]), 'demo.wlt: check: Unknown binding.');
});

async function realCompiler() {
  const bytes = await readFile(new URL('../src/compiler/wavelet_compiler.wasm', import.meta.url));
  return createCompilerRuntime(bytes, {instantiate: (component, imports) => instantiateComponent(component, imports, importModule)});
}

test('REPL returns every top-level value with its source line and shared state', async () => {
  const runtime = await realCompiler();
  const result = await runtime.evaluate({
    definitions: 'Def n cell-new(0)\nDef square Fn {x: s64} mul(x x)\n',
    source: '\n"☃"\n\nadd(\n  20 22) square(3)\ncell-set(n add(cell-get(n) 1))\ncell-get(n)\ncell-get(n)\nDef answer 42',
    repl: true,
  });
  assert.deepEqual(result.results, [
    {line: 2, value: '"☃"'},
    {line: 4, value: '42'},
    {line: 5, value: '9'},
    {line: 6, value: '{}'},
    {line: 7, value: '1'},
    {line: 8, value: '1'},
    {line: 9, value: '{}'},
  ]);
});

test('REPL preserves macro call locations and handles empty or declaration-only inputs', async () => {
  const runtime = await realCompiler();
  const result = await runtime.evaluate({
    source: 'DefMacro twice {x} Quasi add(Unquote(x) Unquote(x))\n\nTwice\n  21\nTwice 3',
    repl: true,
  });
  assert.deepEqual(result.results, [{line: 1, value: '{}'}, {line: 3, value: '42'}, {line: 5, value: '6'}]);
  assert.deepEqual((await runtime.evaluate({source: '', repl: true})).results, []);
  assert.deepEqual((await runtime.evaluate({source: 'Def answer 42', repl: true})).results, [{line: 1, value: '{}'}]);
  await assert.rejects(runtime.evaluate({source: 'Def bad div(1 0)', repl: true}));
});

test('compiler component expands a supplied macro component through owned host wrappers', async () => {
  const runtime = await realCompiler();
  const contents = await readFile(new URL('../../tests/fixtures/produced-macros.wasm', import.meta.url));
  const input = {
    source: 'Import {pkg: "demo:macros/lib@0.1.0" macros: true from: "macros.wasm"}\nIdentity 42',
    macroComponents: [{name: 'macros.wasm', contents}],
  };
  assert.equal((await runtime.evaluate(input)).value, '42');
  assert.equal((await runtime.evaluate(input)).value, '42');
});

test('compiler component accepts arbitrary WIT dependencies and explicit runtime imports', async () => {
  const runtime = await realCompiler();
  const compilation = await runtime.compile({
    sources: [{name: 'custom.wlt', contents: 'Package "demo:consumer@1.0.0"\nImport {pkg: "custom:arithmetic/api@2.3.4" as: arithmetic}\nExport {name: answer result: s64}\nDef answer Fn {} arithmetic/increment(41)'}],
    dependencies: [{name: 'arithmetic.wit', contents: new TextEncoder().encode('package custom:arithmetic@2.3.4; interface api { increment: func(value: s64) -> s64; }')}],
    mode: 'project',
    entry: undefined,
  });
  const bytes = compilation.artifacts.find((artifact) => artifact.name === compilation.entry).bytes;
  await assert.rejects(instantiateComponent(bytes, {}, importModule), /Missing component imports/);
  const component = await instantiateComponent(bytes, {'custom:arithmetic/api': {increment: (value) => value + 1n}}, importModule);
  assert.equal(component['demo:consumer/api@1.0.0'].answer(), 42n);
});

test('compiler component prepares a macro library supplied as virtual project source', async () => {
  const runtime = await realCompiler();
  const compilation = await runtime.compile({
    sources: [
      {name: 'app.wlt', contents: 'Package "memory:app@1.0.0"\nImport {pkg: "memory:macros/api@1.0.0" macros: true}\nExport {name: answer result: s64}\nDef answer Fn {} Identity 42'},
      {name: 'macros.wlt', contents: 'Package "memory:macros@1.0.0"\nDefMacro identity {x} x'},
    ],
    dependencies: [],
    entry: 'app.wlt',
    mode: 'project',
  });
  const artifact = compilation.artifacts.find((artifact) => artifact.name === compilation.entry);
  const component = await instantiateComponent(artifact.bytes, {}, importModule);
  assert.equal(component['memory:app/api@1.0.0'].answer(), 42n);
});

test('component resources can share names with JavaScript intrinsic classes', async () => {
  const runtime = await realCompiler();
  for (const name of ['set', 'array', 'object', 'promise', 'error', 'type-error', 'data-view', 'number', 'symbol']) {
    const source = `DefResource ${name} { New: Fn {start: u32} cell-new(start) value: Fn {self: ${name}} The u32 cell-get(self) }\nLet {item: ${name}(42)} ${name}/value(item)`;
    assert.equal((await runtime.evaluate({source})).value, '42', name);
  }
});

test('compiler component preserves Unicode and collection semantics', async () => {
  const runtime = await realCompiler();
  for (const [name, entry] of compiledCases) {
    assert.equal((await runtime.evaluate({source: entry.code})).value, entry.value, name);
  }
});
