# Checkpoint 04: Cross-Language Identity

**Status:** Passed

Rust, Python, TypeScript/TSX, JavaScript, and C# analyzers all emit normalized
method records with path, hierarchy, kind, symbol, range, content, and metadata.

Automated tests verify:

- body edits preserve stable keys;
- whitespace-only signature formatting preserves stable keys;
- inserting unrelated symbols above does not renumber existing symbols;
- C# overload keys are distinct and survive declaration reordering;
- nested types disambiguate duplicate method names;
- deleted declarations no longer produce their old key.

Overloads receive a suffix derived from a whitespace-normalized signature only
when a base key is duplicated. Ordinary keys retain the readable
`path::container:name::kind:name` form.

