// The documentation site, published at https://docs.ohmyharness.sh.
// Everything specific to omh is in this file; the pages are in
// src/content/docs/, one folder per section listed in `sidebar` below.
//
// tests/docs.rs reads this file: the address the site is published at, where
// an edit is sent, and the folders the sidebar lists. A page in a folder that
// is not listed here is a page the sidebar never shows, and that test says so.
import starlight from '@astrojs/starlight'
import { defineConfig } from 'astro/config'

export default defineConfig({
  site: 'https://docs.ohmyharness.sh',
  integrations: [
    starlight({
      title: 'omh',
      description:
        'The safe way to run a coding agent against your repo: a sandboxed session, your setup already in it, and a branch you can review.',
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/mindsers/ohmyharness' }],
      editLink: {
        baseUrl: 'https://github.com/mindsers/ohmyharness/edit/main/docs/',
      },
      lastUpdated: true,
      sidebar: [
        { label: 'Getting started', items: [{ autogenerate: { directory: 'getting-started' } }] },
        { label: 'Guides', items: [{ autogenerate: { directory: 'guides' } }] },
        { label: 'Reference', items: [{ autogenerate: { directory: 'reference' } }] },
        { label: 'Concepts', items: [{ autogenerate: { directory: 'concepts' } }] },
        { label: 'Project', items: [{ autogenerate: { directory: 'project' } }] },
      ],
    }),
  ],
})
