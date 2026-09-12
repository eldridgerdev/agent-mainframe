# AMF site

Marketing landing page + docs for Agent Mainframe, built with
[Zola](https://www.getzola.org/) (a Rust static site generator — no Node/npm
toolchain).

## Layout

```
site/
  config.toml          # site config (title, base_url, markdown highlighting)
  content/
    _index.md          # landing page copy (rendered by templates/index.html)
    docs/
      _index.md         # docs landing/listing (weight-ordered nav)
      installation.md
      quick-start.md
      core-concepts.md
      learning-mode.md
      configuration.md
  templates/            # Tera templates (base, index, docs list, docs page)
  sass/style.scss       # single stylesheet, compiled to /style.css
  static/               # favicon, images (drop screenshots/GIFs here)
```

Docs content here is written for **end users** and is separate from
`docs/development/*.md`, which documents implementation details for AI
coding agents working in this repo.

## Local development

Install Zola (`brew install zola`, or download a binary from
[GitHub Releases](https://github.com/getzola/zola/releases) — this repo does
not vendor it). Then, from `site/`:

```bash
zola serve
```

This serves the site at `http://127.0.0.1:1111` and rebuilds on save.

```bash
zola build   # writes to site/public/
zola check   # validates internal/external links, no output written
```

## Adding a doc page

Drop a new `content/docs/<slug>.md` with front matter:

```toml
+++
title = "Page Title"
description = "One-line summary for the docs index."
weight = 60   # controls ordering in the sidebar/index
+++
```

It picks up the shared docs layout and sidebar automatically (`page_template`
is set once in `content/docs/_index.md`).

## Hero image

The homepage hero shows a placeholder until `content/_index.md` sets
`extra.hero_image` to a path under `static/` (e.g. `images/dashboard.png`).
Use the `amf-screenshot` skill to capture a real dashboard screenshot or GIF
from a live AMF instance rather than a mockup.

## Deployment

`.github/workflows/site.yml` builds the site with Zola and deploys
`site/public/` to Cloudflare Pages on every push to `main` that touches
`site/**`. It needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`
repository secrets, plus a Cloudflare Pages project (this can reuse the same
Cloudflare account already used for screenshot review previews — see
`scripts/dev/screenshot/publish-pages.sh`). Set `projectName` in the workflow
to that Pages project before enabling it, and update `base_url` in
`config.toml` to the real domain.

Update the `title` favicon and `og`/social copy in `templates/base.html` if
the domain or branding changes.
