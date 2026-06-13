import siteConfig from '@generated/docusaurus.config';
import {registerWavelet} from '@site/src/prism/wavelet';

// Swizzled from @docusaurus/theme-classic to register Wavelet as a Prism
// language for static ```wavelet code blocks. The default behaviour (loading
// every language listed in themeConfig.prism.additionalLanguages) is preserved.
export default function prismIncludeLanguages(PrismObject) {
  const {
    themeConfig: {prism},
  } = siteConfig;
  const {additionalLanguages} = prism;

  // Prism components work on the Prism instance on the window, while
  // prism-react-renderer uses its own Prism instance. We temporarily mount the
  // instance onto window, import components to enhance it, then remove it to
  // avoid polluting the global namespace.
  globalThis.Prism = PrismObject;

  additionalLanguages.forEach((lang) => {
    if (lang === 'php') {
      // eslint-disable-next-line global-require
      require('prismjs/components/prism-markup-templating.js');
    }
    // eslint-disable-next-line global-require, import/no-dynamic-require
    require(`prismjs/components/prism-${lang}`);
  });

  // Wavelet is not a bundled Prism language; register our hand-written grammar.
  registerWavelet(PrismObject);

  delete globalThis.Prism;
}
