import React, {useEffect, useState} from 'react';
import BrowserOnly from '@docusaurus/BrowserOnly';
import examples from '@site/examples.json';
import SourceEditor from './SourceEditor';
import Result from './Result';
import useCompiledRun from './useCompiledRun';
import styles from './styles.module.css';

function Editor({initial, autoRun, rows, lineResults}) {
  const [input, setInput] = useState(initial);
  const execution = useCompiledRun();
  const {run} = execution;
  useEffect(() => {
    if (autoRun) run(initial);
  }, [autoRun, initial, run]);
  const files = input.sources ?? [{name: 'Wavelet', contents: input.source}];
  return (
    <div className={styles.playground} data-state={execution.status}>
      <div className={styles.toolbar}>
        <span className={styles.label}>{input.sources ? 'Wavelet project' : 'Wavelet'}</span>
        <div className={styles.actions}>
          {execution.pending && <button type="button" className={styles.cancelButton} onClick={execution.cancel}>Cancel</button>}
          <button type="button" className={styles.runButton} disabled={execution.pending} onClick={() => run(input)}>
            {execution.pending ? (execution.status === 'running' ? 'Running…' : 'Compiling…') : '▶ Run'}
          </button>
        </div>
      </div>
      {files.map((file, index) => <React.Fragment key={file.name}>
        {input.sources && <div className={styles.toolbar}><span className={styles.label}>{file.name}</span></div>}
        <SourceEditor value={file.contents} rows={rows} lineNumbers={lineResults} label={input.sources ? `Editable ${file.name}` : 'Editable Wavelet source'} onRun={() => run(input)} onChange={(value) => {
          execution.clear();
          setInput(input.sources
            ? {...input, sources: input.sources.map((source, current) => current === index ? {...source, contents: value} : source)}
            : {...input, source: value});
        }} />
      </React.Fragment>)}
      <Result result={execution.result} />
    </div>
  );
}

export default function Playground({id, code, autoRun = false, rows, lineResults = false}) {
  const entry = id ? examples[id] : {code: code ?? ''};
  if (!entry) throw new Error(`Unknown Playground example: ${id}. Regenerate docs/examples.json after adding it.`);
  const initial = React.useMemo(() => entry.sources
    ? {sources: entry.sources, entry: entry.entry, invocation: entry.invocation}
    : {source: entry.code.trimEnd(), repl: lineResults}, [entry.sources, entry.entry, entry.invocation, entry.code, lineResults]);
  const files = initial.sources ?? [{name: 'Wavelet', contents: initial.source}];
  return (
    <BrowserOnly fallback={<div className={styles.playground}>{files.map((file) => <React.Fragment key={file.name}>
      {initial.sources && <div className={styles.toolbar}><span className={styles.label}>{file.name}</span></div>}
      <pre className={styles.editor}>{file.contents}</pre>
    </React.Fragment>)}</div>}>
      {() => <Editor initial={initial} autoRun={autoRun} rows={rows} lineResults={lineResults} />}
    </BrowserOnly>
  );
}
