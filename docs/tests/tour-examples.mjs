import {readFile} from 'node:fs/promises';

const source = await readFile(new URL('../docs/learn-wavelet.mdx', import.meta.url), 'utf8');
export const tourBlocks = Array.from(source.matchAll(/```wavelet(?: title="([^"]+)")?\n([\s\S]*?)```/g), ([, name, contents]) => ({name, contents}));
export const tourValues = ['(true, 3, ("wavelet", 42))', '[11, 12, 13]', '42', '5050', '6', '42', '"concrete"'];
