const DEFAULT_TIMEOUT = 30_000;

export class ExecutionCancelled extends Error {
  constructor(message = 'Execution cancelled.') {
    super(message);
    this.name = 'ExecutionCancelled';
  }
}

export function createWorkerClient(makeWorker, defaultTimeout = DEFAULT_TIMEOUT) {
  let worker;
  let active;
  let nextId = 0;
  let disposed = false;
  const queue = [];

  function resetWorker() {
    worker?.terminate();
    worker = undefined;
  }

  function settle(job, outcome, failed = false) {
    clearTimeout(job.timer);
    job.signal?.removeEventListener('abort', job.abort);
    if (active === job) active = undefined;
    if (failed) job.reject(outcome);
    else job.resolve(outcome);
    pump();
  }

  function failWorker(error) {
    const job = active;
    resetWorker();
    if (job) settle(job, error, true);
  }

  function ensureWorker() {
    if (worker) return;
    const current = makeWorker();
    worker = current;
    current.onmessage = ({data}) => {
      if (worker !== current || !active || data.id !== active.id) return;
      if (data.status) active.onStatus?.(data.status);
      if (data.result) settle(active, data.result);
    };
    current.onerror = (event) => {
      if (worker !== current) return;
      event.preventDefault?.();
      failWorker(new Error(event.message || 'The compiler worker failed.'));
    };
    current.onmessageerror = () => {
      if (worker === current) failWorker(new Error('The compiler returned an unreadable response.'));
    };
  }

  function pump() {
    if (disposed || active || !queue.length) return;
    active = queue.shift();
    const job = active;
    try {
      ensureWorker();
      job.onStatus?.('compiling');
      job.timer = setTimeout(() => {
        if (active !== job) return;
        failWorker(new Error(`Execution exceeded ${job.timeoutMs / 1000} seconds. The worker was stopped; you can run another expression.`));
      }, job.timeoutMs);
      worker.postMessage({id: job.id, input: job.input});
    } catch (error) {
      failWorker(error);
    }
  }

  function evaluate(input, {signal, onStatus, timeoutMs = defaultTimeout} = {}) {
    if (disposed) return Promise.reject(new Error('The execution client has been closed.'));
    if (signal?.aborted) return Promise.reject(new ExecutionCancelled());
    return new Promise((resolve, reject) => {
      const job = {id: ++nextId, input, signal, onStatus, timeoutMs, resolve, reject};
      job.abort = () => {
        if (active === job) {
          resetWorker();
          settle(job, new ExecutionCancelled(), true);
        } else {
          const index = queue.indexOf(job);
          if (index !== -1) {
            queue.splice(index, 1);
            settle(job, new ExecutionCancelled(), true);
          }
        }
      };
      signal?.addEventListener('abort', job.abort, {once: true});
      queue.push(job);
      onStatus?.('queued');
      pump();
    });
  }

  function dispose() {
    disposed = true;
    resetWorker();
    for (const job of [...queue.splice(0), ...(active ? [active] : [])]) {
      settle(job, new ExecutionCancelled('The execution client was closed.'), true);
    }
  }

  return {evaluate, dispose};
}
