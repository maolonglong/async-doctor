# m2_pil_param_branch

PIL `Image.open` / `Image.save` touch `builtins.open` / `os.remove` only under
`if is_path(fp):`.

- BytesIO / StringIO call sites must not be findings or blocking-file candidates.
- Path-string call sites are not provable findings (branch guard on third-party
  sink) — they must surface as **candidates**, not silent misses and not
  unconditional findings.
