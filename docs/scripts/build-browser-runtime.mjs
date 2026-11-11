import {build} from 'esbuild';
import {copyFile, mkdir, readFile, writeFile} from 'node:fs/promises';
import {dirname, join} from 'node:path';
import {fileURLToPath} from 'node:url';

const docs = dirname(dirname(fileURLToPath(import.meta.url)));
const output = join(docs, 'static/runtime');
const compiler = join(docs, 'src/compiler/wavelet_compiler.wasm');
await mkdir(output, {recursive: true});
try {
  await copyFile(compiler, join(output, 'compiler.wasm'));
} catch (error) {
  throw new Error('Build the Wavelet compiler component before building the browser runtime.', {cause: error});
}
await build({
  entryPoints: [join(docs, 'src/runtime/compiler.worker.mjs')],
  outfile: join(output, 'worker.js'),
  bundle: true,
  format: 'esm',
  platform: 'browser',
  target: 'es2022',
  external: ['node:*'],
  minify: true,
  legalComments: 'none',
  sourcemap: false,
});
const bindgen = join(docs, 'node_modules/@bytecodealliance/jco-transpile/vendor');
for (const file of ['js-component-bindgen-component.core.wasm', 'js-component-bindgen-component.core2.wasm']) {
  await copyFile(join(bindgen, file), join(output, file));
}
const licenses = [];
for (const name of ['jco', 'jco-transpile', 'preview2-shim']) {
  const directory = join(docs, 'node_modules/@bytecodealliance', name);
  for (const filename of ['LICENSE', 'LICENSE-APACHE', 'LICENSE-MIT', 'LICENSE.md']) {
    try {
      licenses.push(`${name}: ${filename}\n\n${await readFile(join(directory, filename), 'utf8')}`);
    } catch (error) {
      if (error.code !== 'ENOENT') throw error;
    }
  }
}
await writeFile(join(output, 'LICENSES.txt'), licenses.join('\n\n'));
console.log(`Built browser runtime in ${output}`);
