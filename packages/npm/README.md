# denju-cli

Thin npm installer for the native Denju CLI. It downloads the shared Denju release manifest plus
the matching GitHub Release binary, verifies exact size and SHA-256, and then launches only that
native executable. There is no source-build fallback.

Current npm releases require explicit approval for dependency lifecycle scripts. Install with:

```bash
npm install -g --allow-scripts=denju-cli denju-cli
```

This approves only Denju's installer script rather than weakening npm's global script policy.
