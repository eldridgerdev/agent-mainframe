# Syntax Diff Fixtures

This directory contains small source files used to test syntax
highlighting in the AMF diff viewer.

Files:

- `syntax-test.js`
- `syntax-test.ts`
- `syntax-test.tsx`

Recommended usage:

1. Edit one of these files to create a diff.
2. Open the AMF diff viewer for the current branch.
3. Verify token coloring for added, removed, and context lines.
4. Repeat with each language variant when testing parser installs or
   diff rendering changes.

These files are intentionally standalone and do not need to compile as
part of the Rust project.
