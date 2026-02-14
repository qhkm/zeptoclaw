import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

export default defineConfig({
  integrations: [
    starlight({
      title: 'ZeptoClaw',
      description: 'Ultra-lightweight AI agent framework. Streaming, swarms, plugins — all in ~5MB of Rust.',
      logo: {
        light: './src/assets/logo.svg',
        dark: './src/assets/logo.svg',
        replacesTitle: true,
      },
      social: {
        github: 'https://github.com/qhkm/zeptoclaw',
      },
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
            { label: 'Agent Loop', link: '/concepts/agent-loop/' },
            { label: 'Tools', link: '/concepts/tools/' },
            { label: 'Channels', link: '/concepts/channels/' },
            { label: 'Providers', link: '/concepts/providers/' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'CLI', link: '/reference/cli/' },
            { label: 'Configuration', link: '/reference/configuration/' },
            { label: 'Environment Variables', link: '/reference/environment/' },
            { label: 'Tools Reference', link: '/reference/tools/' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'Plugins', link: '/guides/plugins/' },
            { label: 'Agent Templates', link: '/guides/templates/' },
            { label: 'Deployment', link: '/guides/deployment/' },
            { label: 'Security', link: '/guides/security/' },
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
  site: 'https://zeptoclaw.pages.dev',
  base: '/docs',
});
