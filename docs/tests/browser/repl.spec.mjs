import {compiledCases} from '../compiled-cases.mjs';
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

test('REPL numbers input lines and retains all labeled results in history', async ({page}) => {
  await page.goto('repl');
  const source = '\nadd(\n  20 22) square(3)\n\n"hello"\n';
  const result = await expression(page, source);
  await expect(result.locator('pre')).toHaveText(['2 ⇒ 42', '3 ⇒ 9', '5 ⇒ "hello"']);
  const input = page.getByLabel('REPL expression', {exact: true});
  const editor = input.locator('..');
  await expect(editor.getByTestId('line-numbers')).toHaveText('1\n2\n3\n4\n5\n6');
  await page.getByRole('region', {name: 'REPL history'}).locator('summary').click();
  await expect(page.getByRole('region', {name: 'REPL history'}).getByTestId('wavelet-result').locator('pre')).toHaveText(['2 ⇒ 42', '3 ⇒ 9', '5 ⇒ "hello"']);
  await input.fill(Array.from({length: 30}, (_, index) => String(index)).join('\n'));
  await input.evaluate((element) => { element.scrollTop = element.scrollHeight; element.dispatchEvent(new Event('scroll')); });
  const inputScroll = await input.evaluate((element) => element.scrollTop);
  expect(inputScroll).toBeGreaterThan(0);
  await expect.poll(() => editor.getByTestId('line-numbers').evaluate((element) => element.scrollTop)).toBe(inputScroll);
  await input.fill('42');
  await expect(editor.getByTestId('line-numbers')).toHaveText('1');
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

test('documentation examples execute through the compiler worker', async ({page}) => {
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
        worker.postMessage({id: request, input: {...example, source: example.code}});
      });
      if (example.error ? result.ok : !result.ok || result.value !== example.value || result.output !== example.output) {
        failures.push({name, expected: example, actual: result});
      }
    }
    worker.terminate();
    return failures;
  }, [...Object.entries(examples), ...compiledCases]);
  expect(failures).toEqual([]);
});

test('the tour runs every embedded example and uses edits from both component files', async ({page}) => {
  test.setTimeout(240_000);
  await page.goto('learn-wavelet');
  const runs = page.getByRole('button', {name: '▶ Run', exact: true});
  const tour = Object.entries(examples).filter(([id]) => id.startsWith('tour-'));
  await expect(runs).toHaveCount(tour.length);
  await expect(page.getByRole('textbox')).toHaveCount(9);
  for (const [index, [, example]] of tour.entries()) {
    await runs.nth(index).click();
    await expect(page.getByTestId('wavelet-result').nth(index)).toHaveText(`⇒ ${example.value}`);
  }
  const greeting = page.getByLabel('Editable greeting.wlt', {exact: true});
  const main = page.getByLabel('Editable main.wlt', {exact: true});
  await greeting.fill((await greeting.inputValue()).replace('Hello, ', 'Welcome, '));
  await main.fill((await main.inputValue()).replace('"components"', '"Wavelet"'));
  await runs.last().click();
  await expect(page.getByTestId('wavelet-result').last()).toHaveText('⇒ "Welcome, Wavelet!"');
});
