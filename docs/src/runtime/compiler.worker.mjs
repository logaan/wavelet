import {createCompilerRuntime, formatError} from './compilerRuntime.mjs';

let runtime;

function loadRuntime() {
  if (!runtime) {
    runtime = fetch(new URL('./compiler.wasm', import.meta.url))
      .then((response) => {
        if (!response.ok) throw new Error(`Could not load the compiler (${response.status}).`);
        return response.arrayBuffer();
      })
      .then((bytes) => createCompilerRuntime(new Uint8Array(bytes)))
      .catch((error) => {
        runtime = undefined;
        throw error;
      });
  }
  return runtime;
}

self.onmessage = async ({data}) => {
  const {id, input} = data;
  try {
    self.postMessage({id, status: 'compiling'});
    const compiler = await loadRuntime();
    const result = await compiler.evaluate(input, () => self.postMessage({id, status: 'running'}));
    self.postMessage({id, result});
  } catch (error) {
    self.postMessage({id, result: {ok: false, value: '', output: '', error: formatError(error)}});
  }
};
