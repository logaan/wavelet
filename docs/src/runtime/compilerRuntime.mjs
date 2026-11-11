import {exportedFunction, exportedInterface, instantiateComponent} from './componentLoader.mjs';

const COMPILER_INTERFACE = 'wavelet:compiler/compiler@0.1.0';
const MACRO_INTERFACE = 'wavelet:meta/macros@0.1.0';
const MACRO_HOST = 'wavelet:compiler/macro-host';

class MacroLibrary {
  constructor(exports) {
    this.exports = exportedInterface(exports, MACRO_INTERFACE);
  }

  manifest() {
    return this.exports.manifest();
  }

  expand(name, args) {
    return this.exports.expand(name, args);
  }
}

export function formatError(error) {
  if (Array.isArray(error)) {
    return error.map((diagnostic) => {
      const source = diagnostic.source ? `${diagnostic.source}: ` : '';
      const phase = diagnostic.phase ? `${diagnostic.phase}: ` : '';
      return `${source}${phase}${diagnostic.message ?? String(diagnostic)}`;
    }).join('\n');
  }
  return error?.message ?? String(error);
}

function suppliedMacro(request, dependencies) {
  const imported = request.source.val;
  const candidates = dependencies.filter((file) =>
    (imported.sourcePath && file.name === imported.sourcePath) || file.packageId === imported.packageId,
  );
  if (candidates.length !== 1) {
    throw new Error(`Supply exactly one macro component for ${imported.packageId}${imported.sourcePath ? ` (${imported.sourcePath})` : ''}.`);
  }
  return candidates[0].contents;
}

export async function createCompilerRuntime(compilerBytes, options = {}) {
  const instantiate = options.instantiate ?? instantiateComponent;
  const compilerComponent = await instantiate(compilerBytes, {
    [MACRO_HOST]: {Library: MacroLibrary},
  });
  const compiler = exportedInterface(compilerComponent, COMPILER_INTERFACE);

  async function compile(request, macroComponents = []) {
    const bindings = [];
    const resolved = new Set();
    for (let pass = 0; pass < 64; pass++) {
      const step = compiler.compile(request, bindings.map(({key, component}) => ({key, library: new MacroLibrary(component)})));
      if (step.tag === 'complete') return step.val;
      if (step.tag !== 'needs-macros' || !step.val.length) {
        throw new Error('Compiler returned an invalid macro-resolution step.');
      }
      for (const needed of step.val) {
        if (resolved.has(needed.key)) {
          throw new Error(`Compiler requested an already supplied macro: ${needed.key}`);
        }
        const bytes = needed.source.tag === 'component'
          ? needed.source.val
          : suppliedMacro(needed, macroComponents);
        const component = await instantiate(bytes);
        bindings.push({key: needed.key, component});
        resolved.add(needed.key);
      }
    }
    throw new Error('Macro resolution exceeded 64 compilation passes.');
  }

  async function evaluate({source, definitions = '', dependencies = [], macroComponents = []}, onExecuting) {
    const compilation = await compile({
      sources: [{name: 'playground.wlt', contents: `${definitions}\n${source}`}],
      dependencies,
      entry: undefined,
      mode: 'snippet',
    }, macroComponents);
    const artifact = compilation.artifacts.find((entry) => entry.name === compilation.entry);
    if (!artifact || !compilation.invocation) {
      throw new Error('Compilation did not produce an executable expression.');
    }
    const component = await instantiate(artifact.bytes);
    const invoke = exportedFunction(component, compilation.invocation);
    onExecuting?.();
    const value = invoke();
    if (typeof value !== 'string') throw new Error('The expression did not return its display value.');
    return {ok: true, value, output: '', error: ''};
  }

  return {compile, evaluate};
}
