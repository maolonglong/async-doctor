# m2_pil_param_branch

PIL `Image.open` / `Image.save` touch `builtins.open` / `os.remove` only under
`if is_path(fp):`. Call sites with BytesIO must not be findings; at most
conditional candidates.
