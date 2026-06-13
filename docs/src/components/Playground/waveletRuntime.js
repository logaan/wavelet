// Lazily loads the Wavelet interpreter (compiled to WebAssembly) exactly once
// and returns a `run(src) -> {ok, value, output, error}` function. The wasm is
// the same tree-walking interpreter that backs `wavelet run`.

let runtimePromise = null;

export function loadWavelet() {
  if (!runtimePromise) {
    runtimePromise = (async () => {
      const mod = await import('@site/src/wasm/wavelet.js');
      // No-arg init resolves `new URL('wavelet_bg.wasm', import.meta.url)`,
      // which webpack emits as an asset and serves under the site base URL.
      await mod.default();
      return (src) => JSON.parse(mod._eval(src));
    })();
  }
  return runtimePromise;
}
