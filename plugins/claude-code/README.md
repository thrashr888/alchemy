# Alchemy plugin for Claude Code

The notebook tools and the skill that teaches Claude how to use them, in one
install:

```bash
claude plugin marketplace add thrashr888/alchemy
claude plugin install alchemy@alchemy
```

The server is a small stdio bridge (`server/index.mjs`, Node 18+, no
dependencies) to the Alchemy running on this Mac. It reads the app's own
discovery file for the port and private token, so nothing here is baked in
and Alchemy has to be open for the tools to answer.

`skills/alchemy/SKILL.md` and `server/index.mjs` are copies of
`skills/alchemy/SKILL.md` and `skills/alchemy-mcpb/server/index.mjs` at the
repository root; a test in `src-tauri/src/connectors.rs` keeps them identical.
