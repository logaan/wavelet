import React, {useState} from 'react';
import SourceEditor from './SourceEditor';
import Result from './Result';
import DependencyInputs from './DependencyInputs';
import useCompiledRun from './useCompiledRun';
import styles from './styles.module.css';

export default function Repl() {
  const [definitions, setDefinitions] = useState('Def square Fn {n: s64} mul(n n)');
  const [source, setSource] = useState('square(9)');
  const [history, setHistory] = useState([]);
  const [dependencies, setDependencies] = useState([]);
  const [macroComponents, setMacroComponents] = useState([]);
  const execution = useCompiledRun();
  async function run() {
    const result = await execution.run({source, definitions, dependencies, macroComponents, repl: true});
    if (result) setHistory((entries) => [...entries.slice(-19), {source, definitions, result}]);
  }
  function edit(setter, value) {
    execution.clear();
    setter(value);
  }
  return (
    <section aria-label="Browser REPL" data-state={execution.status}>
      <p>Keep reusable definitions above the expressions. Results show each expression’s starting line. Each run starts a fresh instance: changes to cells and resources last for that run only.</p>
      <DependencyInputs dependencies={dependencies} macros={macroComponents} onDependencies={(value) => edit(setDependencies, value)} onMacros={(value) => edit(setMacroComponents, value)} />
      <div className={styles.playground}>
        <div className={styles.toolbar}><label className={styles.label}>Definitions</label></div>
        <SourceEditor value={definitions} onChange={(value) => edit(setDefinitions, value)} onRun={run} rows={6} label="REPL definitions" lineNumbers />
      </div>
      <div className={styles.playground}>
        <div className={styles.toolbar}>
          <span className={styles.label}>Expressions</span>
          <div className={styles.actions}>
            {execution.pending && <button type="button" className={styles.cancelButton} onClick={execution.cancel}>Cancel</button>}
            <button type="button" className={styles.runButton} disabled={execution.pending} onClick={run}>{execution.pending ? (execution.status === 'running' ? 'Running…' : 'Compiling…') : 'Compile & run'}</button>
          </div>
        </div>
        <SourceEditor value={source} onChange={(value) => edit(setSource, value)} onRun={run} rows={4} label="REPL expression" lineNumbers />
        <Result result={execution.result} />
      </div>
      {history.length > 0 && <section aria-label="REPL history">
        <div className={styles.toolbar}>
          <h2>History</h2>
          <button type="button" className={styles.cancelButton} onClick={() => setHistory([])}>Clear history</button>
        </div>
        {history.map((entry, index) => <details key={index} className={styles.playground}>
          <summary><code>{entry.source}</code></summary>
          <pre className={styles.editor}>{entry.definitions}</pre>
          <Result result={entry.result} />
        </details>)}
      </section>}
    </section>
  );
}
