// @ts-check
// `@type` JSDoc annotations allow editor autocompletion and type checking
// (when paired with `@ts-check`).

import {themes as prismThemes} from 'prism-react-renderer';

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'Wavelet',
  tagline: 'A small homoiconic language for the WebAssembly Component Model',
  favicon: 'img/favicon.ico',

  future: {
    v4: true,
  },

  url: 'https://logaan.github.io',
  baseUrl: '/wavelet/',

  organizationName: 'logaan',
  projectName: 'wavelet',
  trailingSlash: false,

  onBrokenLinks: 'throw',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      /** @type {import('@docusaurus/preset-classic').Options} */
      ({
        docs: {
          sidebarPath: './sidebars.js',

          routeBasePath: '/',
          editUrl: 'https://github.com/logaan/wavelet/tree/main/docs/',
        },
        blog: false,
        theme: {
          customCss: './src/css/custom.css',
        },
      }),
    ],
  ],

  themeConfig:
    /** @type {import('@docusaurus/preset-classic').ThemeConfig} */
    ({
      image: 'img/docusaurus-social-card.jpg',
      colorMode: {
        respectPrefersColorScheme: true,
      },
      navbar: {
        title: 'Wavelet',
        logo: {
          alt: 'Wavelet',
          src: 'img/Logo.png',
        },
        items: [
          {
            type: 'docSidebar',
            sidebarId: 'docsSidebar',
            position: 'left',
            label: 'Docs',
          },
          {
            href: 'https://github.com/logaan/wavelet',
            label: 'GitHub',
            position: 'right',
          },
        ],
      },
      footer: {
        style: 'dark',
        links: [
          {
            title: 'Docs',
            items: [
              {label: 'Introduction', to: '/'},
              {label: 'Getting started', to: '/getting-started'},
              {label: 'Special forms', to: '/language/special-forms'},
              {label: 'Standard library', to: '/stdlib'},
            ],
          },
          {
            title: 'Reference',
            items: [
              {label: 'CLI', to: '/cli'},
              {label: 'Components & composition', to: '/components'},
              {label: 'Roadmap', to: '/roadmap'},
            ],
          },
          {
            title: 'More',
            items: [
              {label: 'GitHub', href: 'https://github.com/logaan/wavelet'},
              {
                label: 'WebAssembly Component Model',
                href: 'https://component-model.bytecodealliance.org/',
              },
            ],
          },
        ],
        copyright: `Copyright © ${new Date().getFullYear()} Logan Campbell. Built with Docusaurus.`,
      },
      prism: {
        theme: prismThemes.github,
        darkTheme: prismThemes.dracula,
        additionalLanguages: ['bash', 'toml', 'wasm', 'rust'],
      },
    }),
};

export default config;
