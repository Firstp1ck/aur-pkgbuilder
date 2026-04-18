# summary

Summarise the last changes: what was done, referencing the actual
diff so the picture is accurate (not speculative). Prefer:

```
git log main..HEAD --oneline
git diff --stat main...HEAD
```

Then drill into individual files if their change is non-obvious.
