# AI reply generation metadata

Captured with AMF's isolated screenshot harness at `120x40`. The fixture uses
a cached PR comment and an AI reply draft, then opens the real reply-confirm
dialog without posting to GitHub.

`reply-generation-metadata.png` shows the generation disclosure preview:

- harness and best-effort model;
- estimated token usage and cost, with explicit unavailable fallbacks when the
  isolated fixture has no provider transcript; and
- the stable `drafted by AI via AMF` attribution footer.
