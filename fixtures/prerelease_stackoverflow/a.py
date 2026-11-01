def f(xs, a, b, c, d):
    out = xs
    if a:
        out = (x for x in out if x)
    if b:
        out = (x for x in out if x)
    if c:
        out = (x for x in out if x)
    if d:
        out = (x for x in out if x)
    return tuple(out)
