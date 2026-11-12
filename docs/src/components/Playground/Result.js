import React from 'react';
import clsx from 'clsx';
import styles from './styles.module.css';

export default function Result({result}) {
  if (!result) return null;
  return (
    <div className={styles.output} aria-live="polite" data-testid="wavelet-result">
      {result.output && <pre className={styles.stdout}>{result.output}</pre>}
      {result.ok && result.results ? (
        result.results.length ? result.results.map(({line, value}, index) => (
          <pre key={index} className={styles.value}><span className={styles.arrow}>{line} ⇒ </span>{value}</pre>
        )) : <pre className={styles.muted}>ok</pre>
      ) : result.ok ? (
        <pre className={result.value ? styles.value : styles.muted}>
          {result.value && <span className={styles.arrow}>⇒ </span>}
          {result.value || 'ok'}
        </pre>
      ) : <pre className={clsx(styles.value, styles.error)}>{result.error}</pre>}
    </div>
  );
}
