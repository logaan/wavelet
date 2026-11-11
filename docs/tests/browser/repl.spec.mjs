import {tourBlocks, tourValues} from '../tour-examples.mjs';
import {expect, test} from '@playwright/test';
import {readFile} from 'node:fs/promises';

const examples = JSON.parse(await readFile(new URL('../../examples.json', import.meta.url), 'utf8'));

async function expression(page, source) {
  await page.getByLabel('REPL expression', {exact: true}).fill(source);
  await page.getByRole('button', {name: 'Compile & run', exact: true}).click();
  return page.getByTestId('wavelet-result').first();
}

test('compiled REPL runs definitions and recovers from diagnostics', async ({page}) => {
  await page.goto('repl');
  const result = await expression(page, 'square(9)');
  await expect(result).toContainText('81');
  await expression(page, 'unknown-function(1)');
  await expect(result).toContainText(/unknown|unbound|undefined/i);
  await expression(page, 'add(20 22)');
  await expect(result).toContainText('42');
});

test('cancels an infinite compiled loop without blocking the page', async ({page}) => {
  await page.goto('repl');
  await page.getByLabel('REPL definitions', {exact: true}).fill('');
  await expression(page, 'Def forever Fn {n: s64} If eq(n 0) 0 forever(sub(n 1))\nforever(-1)');
  await expect(page.getByRole('region', {name: 'Browser REPL', exact: true})).toHaveAttribute('data-state', 'running');
  await page.getByRole('button', {name: 'Cancel', exact: true}).click();
  await expect(page.getByTestId('wavelet-result').first()).toContainText('cancelled');
  await expression(page, 'add(6 7)');
  await expect(page.getByTestId('wavelet-result').first()).toContainText('13');
});

test('documentation examples and tour snippets execute through the compiler worker', async ({page}) => {
  test.setTimeout(600_000);
  await page.goto('repl');
  const failures = await page.evaluate(async (entries) => {
    const worker = new Worker(new URL('runtime/worker.js', document.baseURI.replace(/repl\/?$/, '')), {type: 'module'});
    const failures = [];
    let id = 0;
    for (const [name, example] of entries) {
      const result = await new Promise((resolve, reject) => {
        const request = ++id;
        const timer = setTimeout(() => reject(new Error(`${name}: timed out`)), 60_000);
        worker.onmessage = ({data}) => {
          if (data.id === request && data.result) {
            clearTimeout(timer);
            resolve(data.result);
          }
        };
        worker.onerror = (event) => { clearTimeout(timer); reject(new Error(event.message)); };
        worker.postMessage({id: request, input: {source: example.code}});
      });
      if (example.error ? result.ok : !result.ok || result.value !== example.value || result.output !== example.output) {
        failures.push({name, expected: example, actual: result});
      }
    }
    worker.terminate();
    return failures;
  }, [...Object.entries(examples), ...tourBlocks.filter((block) => !block.name).map((block, index) => [`tour-${index + 1}`, {code: block.contents, value: tourValues[index], output: ''}])]);
  expect(failures).toEqual([]);
});
