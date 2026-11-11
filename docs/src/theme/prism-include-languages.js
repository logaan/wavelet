import siteConfig from '@generated/docusaurus.config';
import {registerWavelet} from '@site/src/prism/wavelet';

export default function prismIncludeLanguages(PrismObject) {
  const {
    themeConfig: {prism},
  } = siteConfig;
  const {additionalLanguages} = prism;

  globalThis.Prism = PrismObject;

  additionalLanguages.forEach((lang) => {
    if (lang === 'php') {

      require('prismjs/components/prism-markup-templating.js');
    }

    require(`prismjs/components/prism-${lang}`);
  });

  registerWavelet(PrismObject);

  delete globalThis.Prism;
}
