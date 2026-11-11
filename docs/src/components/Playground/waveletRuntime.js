let runtimePromise = null;

export function loadWavelet() {
  if (!runtimePromise) {
    runtimePromise = (async () => {
      const mod = await import('@site/src/wasm/wavelet.js');

      await mod.default();
      return (src) => JSON.parse(mod._eval(src));
    })();
  }
  return runtimePromise;
}
