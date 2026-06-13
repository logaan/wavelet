import React, {useState, useEffect, useRef, useCallback} from 'react';
import BrowserOnly from '@docusaurus/BrowserOnly';
import clsx from 'clsx';
import styles from './styles.module.css';
import examples from '@site/examples.json';

// A live, editable Wavelet example. The code runs in the reader's browser via
// the WebAssembly-compiled interpreter (see waveletRuntime.js).
//
//   <Playground id="std-arith" />          // code from docs/examples.json
//   <Playground id="gs-hello" autoRun />
//   <Playground code={`add[2 3]`} />        // inline (not test-backed)
//
// Prefer `id`: those snippets are the single source of truth in
// docs/examples.json and are verified by the Rust test suite, so they cannot
// drift from the language. `code` is an escape hatch for ad-hoc examples.
//
// Props:
//   id       key into docs/examples.json (test-backed)
//   code     inline source, used when `id` is absent
//   autoRun  evaluate once as soon as the interpreter loads
//   rows     textarea height in rows (defaults to the line count of the code)

function Editor({initial, autoRun, rows}) {
  const [src, setSrc] = useState(initial.replace(/\s+$/, ''));
  const [result, setResult] = useState(null);
  const [status, setStatus] = useState('loading'); // loading | ready | error
  const runRef = useRef(null);

  const doRun = useCallback(
    (codeArg, runArg) => {
      const run = runArg || runRef.current;
      if (!run) return;
      const code = typeof codeArg === 'string' ? codeArg : src;
      try {
        setResult(run(code));
      } catch (e) {
        setResult({ok: false, value: '', output: '', error: String(e)});
      }
    },
    [src],
  );

  useEffect(() => {
    let live = true;
    import('./waveletRuntime')
      .then((m) => m.loadWavelet())
      .then((run) => {
        if (!live) return;
        runRef.current = run;
        setStatus('ready');
        if (autoRun) doRun(src, run);
      })
      .catch((e) => {
        if (!live) return;
        setStatus('error');
        setResult({ok: false, value: '', output: '', error: String(e)});
      });
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const lineCount = src.split('\n').length;
  const height = rows || Math.max(2, Math.min(lineCount, 20));

  const onKeyDown = (e) => {
    // Cmd/Ctrl-Enter runs; Tab inserts two spaces.
    if ((e.metaKey || e.ctrlKey) && e.key === 'Enter') {
      e.preventDefault();
      doRun();
    } else if (e.key === 'Tab') {
      e.preventDefault();
      const el = e.target;
      const {selectionStart: s, selectionEnd: en} = el;
      const next = src.slice(0, s) + '  ' + src.slice(en);
      setSrc(next);
      requestAnimationFrame(() => {
        el.selectionStart = el.selectionEnd = s + 2;
      });
    }
  };

  return (
    <div className={styles.playground}>
      <div className={styles.toolbar}>
        <span className={styles.label}>Wavelet</span>
        <button
          type="button"
          className={styles.runButton}
          disabled={status !== 'ready'}
          onClick={() => doRun()}>
          {status === 'loading' ? 'Loading…' : '▶ Run'}
        </button>
      </div>
      <textarea
        className={styles.editor}
        spellCheck={false}
        rows={height}
        value={src}
        onChange={(e) => setSrc(e.target.value)}
        onKeyDown={onKeyDown}
        aria-label="Editable Wavelet source"
      />
      {result && (
        <div className={styles.output}>
          {result.output && <pre className={styles.stdout}>{result.output}</pre>}
          {result.ok ? (
            result.value !== '' && (
              <pre className={styles.value}>
                <span className={styles.arrow}>⇒ </span>
                {result.value}
              </pre>
            )
          ) : (
            <pre className={clsx(styles.value, styles.error)}>{result.error}</pre>
          )}
          {result.ok && result.value === '' && !result.output && (
            <pre className={styles.muted}>ok</pre>
          )}
        </div>
      )}
    </div>
  );
}

export default function Playground({id, code, autoRun = false, rows}) {
  let source = code ?? '';
  if (id) {
    const entry = examples[id];
    if (!entry) {
      throw new Error(
        `<Playground id="${id}"> not found in docs/examples.json. ` +
          `Add it to docs/scripts/gen-examples.mjs and regenerate.`,
      );
    }
    source = entry.code;
  }
  return (
    <BrowserOnly
      fallback={
        <div className={styles.playground}>
          <pre className={styles.editor}>{source.replace(/\s+$/, '')}</pre>
        </div>
      }>
      {() => <Editor initial={source} autoRun={autoRun} rows={rows} />}
    </BrowserOnly>
  );
}
