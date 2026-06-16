// @ts-check
// `@type` JSDoc annotations allow editor autocompletion and type checking
// (when paired with `@ts-check`).
// See: https://docusaurus.io/docs/api/docusaurus-config

import {themes as prismThemes} from 'prism-react-renderer';

// This runs in Node.js - Don't use client-side code here (browser APIs, JSX...)

/** @type {import('@docusaurus/types').Config} */
const config = {
  title: 'Wavelet',
  tagline: 'A small homoiconic language for the WebAssembly Component Model',
  favicon: 'img/favicon.ico',

  future: {
    v4: true, // Improve compatibility with the upcoming Docusaurus v4
  },

  // Production URL and base path for GitHub Pages project site
  // (served at https://logaan.github.io/wavelet/).
  url: 'https://logaan.github.io',
  baseUrl: '/wavelet/',

  // GitHub pages deployment config.
  organizationName: 'logaan', // GitHub org/user name.
  projectName: 'wavelet', // repo name.
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
          // Docs at the site root: /wavelet/ goes straight to the docs.
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
          src: 'img/logo.svg',
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
