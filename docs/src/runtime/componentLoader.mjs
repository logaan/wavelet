import {transpile} from '@bytecodealliance/jco/component';

export async function importGeneratedModule(source) {
  const url = URL.createObjectURL(new Blob([source], {type: 'text/javascript'}));
  try {
    return await import(url);
  } finally {
    URL.revokeObjectURL(url);
  }
}

export async function instantiateComponent(bytes, imports = {}, importModule = importGeneratedModule) {
  const generated = await transpile(bytes, {
    name: 'component',
    instantiation: {tag: 'async'},
    noTypescript: true,
    noNodejsCompat: true,
    base64Cutoff: 0,
    noComponentErrorWrapping: true,
  });
  const missing = generated.imports.filter((name) => !(name in imports));
  if (missing.length) {
    throw new Error(`Missing component imports: ${missing.join(', ')}`);
  }
  const files = new Map(generated.files);
  const javascript = files.get('component.js');
  if (!javascript) throw new Error('Component transpilation did not produce JavaScript bindings.');
  const bindings = new TextDecoder().decode(javascript).replaceAll(
    '._createdReps = new Set();',
    '._createdReps = new globalThis.Set();',
  );
  const module = await importModule(bindings);
  const modules = new Map();
  const getCoreModule = (path) => {
    const name = path.replace(/^\.\//, '');
    if (!modules.has(name)) {
      const core = files.get(name);
      if (!core) throw new Error(`Missing generated core module: ${name}`);
      modules.set(name, WebAssembly.compile(core));
    }
    return modules.get(name);
  };
  return module.instantiate(getCoreModule, imports);
}

export function exportedInterface(component, name) {
  const result = component[name] ?? component[name.replace(/@[^/]+$/, '')];
  if (!result) throw new Error(`Component does not export ${name}.`);
  return result;
}

export function exportedFunction(component, invocation) {
  const iface = exportedInterface(component, invocation.interfaceName);
  const name = invocation.functionName.replace(/-([a-z])/g, (_, letter) => letter.toUpperCase());
  if (typeof iface[name] !== 'function') {
    throw new Error(`Component does not export ${invocation.functionName}.`);
  }
  return iface[name].bind(iface);
}
