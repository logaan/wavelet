import {tourBlocks as blocks, tourValues as expected} from './tour-examples.mjs';
import {importComponentBindings as importModule} from '../scripts/node-component-loader.mjs';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import test from 'node:test';
import {createCompilerRuntime} from '../src/runtime/compilerRuntime.mjs';
import {instantiateComponent} from '../src/runtime/componentLoader.mjs';

const runtime = await createCompilerRuntime(await readFile(new URL('../src/compiler/wavelet_compiler.wasm', import.meta.url)), {
  instantiate: (bytes, imports) => instantiateComponent(bytes, imports, importModule),
});

const snippets = blocks.filter((block) => !block.name);
assert.equal(snippets.length, expected.length);
for (const [index, block] of snippets.entries()) {
  test(`tour snippet ${index + 1} compiles and evaluates`, async () => {
    const result = await runtime.evaluate({source: block.contents});
    assert.equal(result.value, expected[index]);
  });
}

test('the tour project composes and executes both source files', async () => {
  const compilation = await runtime.compile({sources: blocks.filter((block) => block.name), dependencies: [], entry: 'main.wlt', mode: 'project'});
  const artifact = compilation.artifacts.find((artifact) => artifact.name === compilation.entry);
  const component = await instantiateComponent(artifact.bytes, {}, importModule);
  assert.equal(component['tour:main/api@0.1.0'].run(), 'Hello, components!');
});
