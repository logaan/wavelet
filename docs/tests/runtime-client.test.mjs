import assert from 'node:assert/strict';
import test from 'node:test';
import {createWorkerClient, ExecutionCancelled} from '../src/runtime/workerClient.mjs';

function harness(timeout = 1000) {
  const workers = [];
  const client = createWorkerClient(() => {
    const worker = {
      messages: [],
      terminated: false,
      postMessage(message) { this.messages.push(message); },
      terminate() { this.terminated = true; },
      reply(index, result) { this.onmessage({data: {id: this.messages[index].id, result}}); },
    };
    workers.push(worker);
    return worker;
  }, timeout);
  return {client, workers};
}

test('serializes runs and ignores stale results', async () => {
  const {client, workers} = harness();
  const first = client.evaluate({source: '1'});
  const second = client.evaluate({source: '2'});
  assert.equal(workers[0].messages.length, 1);
  workers[0].reply(0, {value: '1'});
  assert.deepEqual(await first, {value: '1'});
  assert.equal(workers[0].messages.length, 2);
  workers[0].reply(0, {value: 'stale'});
  workers[0].reply(1, {value: '2'});
  assert.deepEqual(await second, {value: '2'});
  client.dispose();
});

test('cancelling an active run kills its worker and preserves queued runs', async () => {
  const {client, workers} = harness();
  const controller = new AbortController();
  const first = client.evaluate({source: 'loop'}, {signal: controller.signal});
  const rejection = assert.rejects(first, ExecutionCancelled);
  const second = client.evaluate({source: '2'});
  controller.abort();
  await rejection;
  assert.equal(workers[0].terminated, true);
  assert.equal(workers.length, 2);
  workers[0].reply(0, {value: 'stale'});
  workers[1].reply(0, {value: '2'});
  assert.deepEqual(await second, {value: '2'});
  client.dispose();
});

test('cancelling a queued run leaves the active worker intact', async () => {
  const {client, workers} = harness();
  const first = client.evaluate({source: '1'});
  const controller = new AbortController();
  const second = client.evaluate({source: '2'}, {signal: controller.signal});
  const rejection = assert.rejects(second, ExecutionCancelled);
  controller.abort();
  await rejection;
  assert.equal(workers[0].terminated, false);
  workers[0].reply(0, {value: '1'});
  await first;
  assert.equal(workers[0].messages.length, 1);
  client.dispose();
});

test('a stalled worker times out and the next run gets a fresh worker', async () => {
  const {client, workers} = harness(20);
  await assert.rejects(client.evaluate({source: 'loop'}), /Execution exceeded/);
  assert.equal(workers[0].terminated, true);
  const next = client.evaluate({source: '3'});
  workers[1].reply(0, {value: '3'});
  assert.deepEqual(await next, {value: '3'});
  client.dispose();
});

test('worker failures reject the active run and disposal rejects queued runs', async () => {
  const {client, workers} = harness();
  const first = client.evaluate({source: '1'});
  const failure = assert.rejects(first, /load failed/);
  workers[0].onerror({message: 'load failed'});
  await failure;
  const second = client.evaluate({source: '2'});
  const third = client.evaluate({source: '3'});
  const stopped = Promise.all([assert.rejects(second, ExecutionCancelled), assert.rejects(third, ExecutionCancelled)]);
  client.dispose();
  await stopped;
  await assert.rejects(client.evaluate({source: '4'}), /closed/);
});
