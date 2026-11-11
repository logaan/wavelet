import React from 'react';
import Layout from '@theme/Layout';
import BrowserOnly from '@docusaurus/BrowserOnly';
import Repl from '@site/src/components/Playground/Repl';

export default function BrowserReplPage() {
  return <Layout title="Browser REPL" description="Compile and run Wavelet expressions in your browser.">
    <main className="container margin-vert--lg">
      <h1>Browser REPL</h1>
      <p>Edit Wavelet source, then compile and run it. Use Ctrl+Enter or ⌘+Enter to run an expression.</p>
      <BrowserOnly fallback={<p>The interactive editor loads in your browser.</p>}>{() => <Repl />}</BrowserOnly>
    </main>
  </Layout>;
}
