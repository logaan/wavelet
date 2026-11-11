import {createWorkerClient} from '../../runtime/workerClient.mjs';

let client;
let workerUrl;

export function loadWavelet(url) {
  if (!client || workerUrl !== url) {
    client?.dispose();
    workerUrl = url;
    client = createWorkerClient(() => new Worker(url, {type: 'module', name: 'wavelet-compiler'}));
  }
  return client;
}
