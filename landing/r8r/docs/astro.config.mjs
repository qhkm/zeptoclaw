import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  integrations: [
    starlight({
      title: 'r8r',
      description: 'Lightning-fast workflow automation for developers. Built in Rust.',
      logo: {
        light: './src/assets/logo.svg',
        dark: './src/assets/logo.svg',
        replacesTitle: true,
      },
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/r8r/r8r' },
      ],
      sidebar: [
        {
          label: 'Getting Started',
          items: [
            { label: 'Introduction', link: '/getting-started/introduction/' },
            { label: 'Installation', link: '/getting-started/installation/' },
            { label: 'Quick Start', link: '/getting-started/quick-start/' },
          ],
        },
        {
          label: 'Core Concepts',
          items: [
            { label: 'Workflows', link: '/concepts/workflows/' },
            { label: 'Nodes', link: '/concepts/nodes/' },
            { label: 'Connections', link: '/concepts/connections/' },
            { label: 'Triggers', link: '/concepts/triggers/' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'CLI', link: '/reference/cli/' },
            { label: 'Configuration', link: '/reference/configuration/' },
            { label: 'Node Types', link: '/reference/node-types/' },
            { label: 'Environment Variables', link: '/reference/environment/' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'Building Custom Nodes', link: '/guides/custom-nodes/' },
            { label: 'API Integration', link: '/guides/api-integration/' },
            { label: 'Error Handling', link: '/guides/error-handling/' },
            { label: 'Deployment', link: '/guides/deployment/' },
          ],
        },
      ],
      customCss: ['./src/styles/custom.css'],
      favicon: '/favicon.svg',
      head: [
        {
          tag: 'meta',
          attrs: { property: 'og:image', content: '/og-image.png' },
        },
      ],
    }),
  ],
  site: 'https://r8r.pages.dev',
});
