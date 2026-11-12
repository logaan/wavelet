import {importComponentBindings as importModule} from '../scripts/node-component-loader.mjs';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
import {createCompilerRuntime} from '../src/runtime/compilerRuntime.mjs';
import {instantiateComponent} from '../src/runtime/componentLoader.mjs';

const examples = JSON.parse(await readFile(new URL('../examples.json', import.meta.url), 'utf8'));
const runtime = await createCompilerRuntime(await readFile(new URL('../src/compiler/wavelet_compiler.wasm', import.meta.url)), {
  instantiate: (bytes, imports) => instantiateComponent(bytes, imports, importModule),
});

for (const [id, example] of Object.entries(examples).filter(([id]) => id.startsWith('tour-'))) {
  test(`${id} compiles and evaluates`, async () => {
    const result = await runtime.evaluate({...example, source: example.code});
    assert.equal(result.value, example.value);
    assert.equal(result.output, example.output);
  });
}
