// @ts-check

/** @type {import('@docusaurus/plugin-content-docs').SidebarsConfig} */
const sidebars = {
  docsSidebar: [
    'intro',
    'philosophy',
    'getting-started',
    {
      type: 'category',
      label: 'The language',
      collapsed: false,
      items: [
        'language/syntax',
        'language/reader-sugar',
        'language/values',
        'language/evaluation',
        'language/special-forms',
        'language/pattern-matching',
        'language/macros',
        'language/tail-calls',
        'language/type-system',
      ],
    },
    'stdlib',
    'components',
    'web-apps',
    'cli',
    'supply-chain',
    'development',
    'roadmap',
  ],
};

export default sidebars;
