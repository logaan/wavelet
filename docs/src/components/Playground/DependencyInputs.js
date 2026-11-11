import React from 'react';

async function filesFrom(input) {
  return Promise.all(Array.from(input.files, async (file) => ({name: file.name, contents: new Uint8Array(await file.arrayBuffer())})));
}

export default function DependencyInputs({dependencies, macros, onDependencies, onMacros}) {
  return <details>
    <summary>Component dependencies</summary>
    <p>Imports use files you provide. Add WIT dependencies for type checking and macro components for compile-time expansion.</p>
    <p><label>WIT dependencies <input type="file" multiple accept=".wit,.wasm,.wat" onChange={async (event) => onDependencies(await filesFrom(event.target))} /></label></p>
    {dependencies.length > 0 && <ul>{dependencies.map((file) => <li key={file.name}>{file.name}</li>)}</ul>}
    <p><label>Macro components <input type="file" multiple accept=".wasm" onChange={async (event) => onMacros(await filesFrom(event.target))} /></label></p>
    {macros.map((file, index) => <p key={file.name}>
      <label>{file.name} package ID <input type="text" placeholder="example:macros" value={file.packageId ?? ''} onChange={(event) => onMacros(macros.map((entry, current) => current === index ? {...entry, packageId: event.target.value} : entry))} /></label>
    </p>)}
  </details>;
}
