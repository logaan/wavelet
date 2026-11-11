import React, {useEffect, useState} from 'react';
import BrowserOnly from '@docusaurus/BrowserOnly';
import examples from '@site/examples.json';
import SourceEditor from './SourceEditor';
import Result from './Result';
import useCompiledRun from './useCompiledRun';
import styles from './styles.module.css';

function Editor({initial, autoRun, rows}) {
  const [source, setSource] = useState(initial.trimEnd());
  const execution = useCompiledRun();
  const {run} = execution;
  useEffect(() => {
    if (autoRun) run({source: initial});
  }, [autoRun, initial, run]);
  return (
    <div className={styles.playground} data-state={execution.status}>
      <div className={styles.toolbar}>
        <span className={styles.label}>Wavelet</span>
        <div className={styles.actions}>
          {execution.pending && <button type="button" className={styles.cancelButton} onClick={execution.cancel}>Cancel</button>}
          <button type="button" className={styles.runButton} disabled={execution.pending} onClick={() => run({source})}>
            {execution.pending ? (execution.status === 'running' ? 'Running…' : 'Compiling…') : '▶ Run'}
          </button>
        </div>
      </div>
      <SourceEditor value={source} rows={rows} onRun={() => run({source})} onChange={(value) => {
        execution.clear();
        setSource(value);
      }} />
      <Result result={execution.result} />
    </div>
  );
}

export default function Playground({id, code, autoRun = false, rows}) {
  let source = code ?? '';
  if (id) {
    const entry = examples[id];
    if (!entry) throw new Error(`Unknown Playground example: ${id}. Regenerate docs/examples.json after adding it.`);
    source = entry.code;
  }
  return (
    <BrowserOnly fallback={<div className={styles.playground}><pre className={styles.editor}>{source.trimEnd()}</pre></div>}>
      {() => <Editor initial={source} autoRun={autoRun} rows={rows} />}
    </BrowserOnly>
  );
}
