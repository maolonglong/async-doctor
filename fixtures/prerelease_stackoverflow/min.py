def f(xs, a):
    out = xs
    if a:
        out = (x for x in out)
    return tuple(out)
