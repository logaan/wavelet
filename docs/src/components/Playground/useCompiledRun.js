import {useCallback, useEffect, useRef, useState} from 'react';
import useBaseUrl from '@docusaurus/useBaseUrl';

export default function useCompiledRun() {
  const workerUrl = useBaseUrl('/runtime/worker.js');
  const [result, setResult] = useState(null);
  const [status, setStatus] = useState('idle');
  const current = useRef(0);
  const controller = useRef(null);

  const clear = useCallback(() => {
    current.current++;
    controller.current?.abort();
    controller.current = null;
    setStatus('idle');
    setResult(null);
  }, []);

  const run = useCallback(async (input) => {
    const id = ++current.current;
    controller.current?.abort();
    const abort = new AbortController();
    controller.current = abort;
    setStatus('loading');
    setResult(null);
    try {
      const {loadWavelet} = await import('./waveletRuntime');
      const client = loadWavelet(workerUrl);
      const outcome = await client.evaluate(input, {
        signal: abort.signal,
        onStatus: (next) => {
          if (current.current === id) setStatus(next);
        },
      });
      if (current.current !== id) return undefined;
      setResult(outcome);
      setStatus('idle');
      return outcome;
    } catch (error) {
      if (current.current !== id) return undefined;
      const outcome = {ok: false, value: '', output: '', error: error.message ?? String(error)};
      setResult(outcome);
      setStatus('idle');
      return outcome;
    }
  }, [workerUrl]);

  const cancel = useCallback(() => {
    controller.current?.abort();
  }, []);

  useEffect(() => () => {
    current.current++;
    controller.current?.abort();
  }, []);

  return {result, status, run, cancel, clear, pending: status !== 'idle'};
}
