# tests

What the build workflow (`.github/workflows/build.yml`) runs against the packaged sill, besides its unit tests:

| | what | gate |
|---|---|---|
| `conformance/` | 56 cases agents rely on from a Bash on Windows (arguments to native programs, paths, encoding, line endings, processes, limits, `#!` scripts, tools, errors), run through `bash.exe -c` and through the MCP server (`bash.exe --mcp`) | every case passes, both ways |
| `compat/` | brush's compat suite (third_party/brush/brush-shell/tests/cases) with Git Bash as the reference, one file at a time in a Job Object | no failure beyond `compat/baseline-windows.txt`, and at least its count of passing cases |

On Linux, the same compat suite runs with bash itself as the reference (`cargo test --test brush-compat-tests`): a case that fails without a `known_failure` mark, or passes with one, fails the build.

`echoargs/` builds the helper that shows what a native program received.
