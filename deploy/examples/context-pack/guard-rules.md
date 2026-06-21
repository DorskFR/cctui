# Guard rules (context pack example)

Shared tool-set and network-set definitions consumed by `cctui-guard`. Each
definition is `[name]: member, member, …`; blank lines and `#` comments are
ignored. Sets may reference other sets (expanded recursively).

This fixture is NEUTRAL — every host is a placeholder (`example.com`,
`internal.invalid`). A real pack lists your actual model gateway, VCS host, etc.

# Tool sets
[code-read]: Read, Grep, Glob, LSP, WebFetch, WebSearch
[code-write]: Edit, Write
[git-read]: git log, git diff, git status, git fetch
[git-write]: git checkout, git commit, git push
[github-read]: gh pr list, gh pr view, gh api
[github-write]: gh pr create, gh pr edit, git push

# Composites
[all-read]: code-read, git-read, github-read
[all-write]: code-write, git-write, github-write
[remote-write]: git push, github-write

# Network sets (host:port; use host:* for all ports)
[net-model]: api.example.com:443, downloads.example.com:443
[net-vcs]: github.example.com:443, github.example.com:22, api.github.example.com:443

# Per-surface oracle network sets — the net-allow each oracle skill needs to
# exercise its surface in-pod. Each is the SANDBOX/test-mode host only; the
# production host of any third party is deliberately absent so an oracle cannot
# touch live data. pure-calc/webhook get no third-party host at all (golden
# files / replayed fixtures are local); frontend/backend reach only the loopback
# dev server (no set needed beyond net-model). A pack replaces every placeholder
# with its provider's actual sandbox host.
[net-external-sandbox]: sandbox.example.com:443
[net-payments-sandbox]: api.sandbox.stripe.example.com:443
