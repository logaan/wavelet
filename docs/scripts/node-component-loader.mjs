import {mkdtemp, rm, writeFile} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import {join} from 'node:path';
import {pathToFileURL} from 'node:url';

export async function importComponentBindings(source) {
  const directory = await mkdtemp(join(tmpdir(), 'wavelet-bindings-'));
  const path = join(directory, 'component.mjs');
  try {
    await writeFile(path, source);
    return await import(pathToFileURL(path).href);
  } finally {
    await rm(directory, {recursive: true, force: true});
  }
}
