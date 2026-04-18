# git-staged-msg

Create or update a commit message (short and long version) for the
staged files. Save to `dev/COMMIT/<branch-name>.md`.

Allowed commit types (each bullet in the body may also use these
prefixes):

- fix:
- feat:
- change:
- perf:
- test:
- chore:
- refactor:
- docs:
- style:
- build:
- ci:
- revert:

Commit structure:

```
<type>: <short subject>

- <type>: <bullet point 1, short and concrete>
- <type>: <bullet point 2>
…
```

(No additional narrative text.)
