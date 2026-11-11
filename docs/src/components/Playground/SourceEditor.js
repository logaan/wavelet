import React, {useRef} from 'react';
import {Highlight, Prism, themes} from 'prism-react-renderer';
import {useColorMode} from '@docusaurus/theme-common';
import clsx from 'clsx';
import {registerWavelet} from '@site/src/prism/wavelet';
import styles from './styles.module.css';

registerWavelet(Prism);

export default function SourceEditor({value, onChange, onRun, label = 'Editable Wavelet source', rows}) {
  const pre = useRef(null);
  const {colorMode} = useColorMode();
  const height = rows ?? Math.max(2, Math.min(value.split('\n').length, 20));
  function onKeyDown(event) {
    if ((event.metaKey || event.ctrlKey) && event.key === 'Enter') {
      event.preventDefault();
      onRun?.();
    } else if (event.key === 'Tab') {
      event.preventDefault();
      const input = event.target;
      const {selectionStart: start, selectionEnd: end} = input;
      onChange(value.slice(0, start) + '  ' + value.slice(end));
      requestAnimationFrame(() => {
        input.selectionStart = input.selectionEnd = start + 2;
      });
    }
  }
  return (
    <div className={styles.editorWrap}>
      <pre ref={pre} className={clsx(styles.editor, styles.highlight)} aria-hidden="true">
        <Highlight prism={Prism} theme={colorMode === 'dark' ? themes.dracula : themes.github} code={value + '\n'} language="wavelet">
          {({tokens, getLineProps, getTokenProps}) => tokens.map((line, index) => (
            <span key={index} {...getLineProps({line})}>
              {line.map((token, key) => <span key={key} {...getTokenProps({token})} />)}
              {index < tokens.length - 1 && '\n'}
            </span>
          ))}
        </Highlight>
      </pre>
      <textarea
        className={clsx(styles.editor, styles.input)}
        spellCheck={false}
        rows={height}
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={onKeyDown}
        onScroll={(event) => {
          if (pre.current) {
            pre.current.scrollTop = event.target.scrollTop;
            pre.current.scrollLeft = event.target.scrollLeft;
          }
        }}
        aria-label={label}
      />
    </div>
  );
}
