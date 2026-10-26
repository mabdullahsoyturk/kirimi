# Changelog fragments

Every user-visible change gets a fragment in this directory. At release time,
towncrier collects them into `CHANGELOG.md` and deletes them.

Name a fragment `<PR number>.<type>.md`, or `+<slug>.<type>.md` if there is no
pull request. Types: `added`, `changed`, `deprecated`, `removed`, `fixed`,
`security`. Write one or two sentences in Markdown, for example
`42.fixed.md`:

```markdown
`MultipartParser.get()` no longer reads the whole stream when the part exists.
```

Create one with `uv run towncrier create 42.fixed.md --edit`.
