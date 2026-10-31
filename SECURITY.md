# Security Policy

## Supported versions

Security fixes are provided for the latest release. Older releases may not receive patches.

## Reporting a vulnerability

Please use [GitHub Security Advisories](https://github.com/iwangjie/Nemorous/security/advisories/new) for private reports. Do not open a public issue for a vulnerability.

Include:
- affected version and platform
- reproduction steps
- impact and required preconditions
- any relevant logs with secrets removed

We will acknowledge the report, investigate, and coordinate a fix and disclosure.

## Scope

In scope:
- credential handling and secret storage
- path traversal or command injection in Git, LSP, AI or plugin operations
- unsafe AI merge or conflict resolution behaviour
- update signature and manifest verification
- privilege or permission boundaries in the plugin runtime

Out of scope:
- vulnerabilities in third-party Git hosting providers or language servers
- issues requiring a compromised local machine or account
- social engineering

## Handling secrets

Nemorous does not write Git HTTPS tokens to its own configuration. Tokens are passed to the configured Git credential helper. SSH and GPG keys remain in their normal system locations. Third-party plugins are responsible for their own storage; review a plugin before storing credentials in it.

## Updates

Release assets include checksums and signed update manifests. Verify downloads when your threat model requires it.
