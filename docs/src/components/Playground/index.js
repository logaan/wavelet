import React, {useState, useEffect, useRef, useCallback} from 'react';
import BrowserOnly from '@docusaurus/BrowserOnly';
import {Highlight, Prism, themes} from 'prism-react-renderer';
import {useColorMode} from '@docusaurus/theme-common';
import clsx from 'clsx';
import styles from './styles.module.css';
import examples from '@site/examples.json';
import {registerWavelet} from '@site/src/prism/wavelet';

registerWavelet(Prism);

function Highlighted({code, theme}) {
  return (
    <Highlight prism={Prism} theme={theme} code={code} language="wavelet">
      {({tokens, getLineProps, getTokenProps}) => (
        <>
          {tokens.map((line, i) => (
            <span key={i} {...getLineProps({line})}>
              {line.map((token, key) => (
                <span key={key} {...getTokenProps({token})} />
              ))}
              {i < tokens.length - 1 && '\n'}
            </span>
          ))}
        </>
      )}
    </Highlight>
  );
}

function Editor({initial, autoRun, rows}) {
  const [src, setSrc] = useState(initial.replace(/\s+$/, ''));
  const [result, setResult] = useState(null);
  const [status, setStatus] = useState('loading');
  const runRef = useRef(null);
  const preRef = useRef(null);
  const {colorMode} = useColorMode();
  const theme = colorMode === 'dark' ? themes.dracula : themes.github;

  const syncScroll = (e) => {
    if (!preRef.current) return;
    preRef.current.scrollTop = e.target.scrollTop;
    preRef.current.scrollLeft = e.target.scrollLeft;
  };

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

  }, []);

  const lineCount = src.split('\n').length;
  const height = rows || Math.max(2, Math.min(lineCount, 20));

  const onKeyDown = (e) => {

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
      <div className={styles.editorWrap}>
        <pre
          ref={preRef}
          className={clsx(styles.editor, styles.highlight)}
          aria-hidden="true">
          <Highlighted code={src + '\n'} theme={theme} />
        </pre>
        <textarea
          className={clsx(styles.editor, styles.input)}
          spellCheck={false}
          rows={height}
          value={src}
          onChange={(e) => setSrc(e.target.value)}
          onKeyDown={onKeyDown}
          onScroll={syncScroll}
          aria-label="Editable Wavelet source"
        />
      </div>
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
