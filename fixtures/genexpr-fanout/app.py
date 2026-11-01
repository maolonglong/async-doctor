"""Exponential genexpr fan-out must finish quickly (memoized expansion)."""

import time


async def handler():
    xs = [1]
    g0 = (time.sleep(0) for _ in xs)
    g1 = (a for a in g0 for b in g0)
    g2 = (a for a in g1 for b in g1)
    g3 = (a for a in g2 for b in g2)
    g4 = (a for a in g3 for b in g3)
    g5 = (a for a in g4 for b in g4)
    g6 = (a for a in g5 for b in g5)
    g7 = (a for a in g6 for b in g6)
    g8 = (a for a in g7 for b in g7)
    g9 = (a for a in g8 for b in g8)
    g10 = (a for a in g9 for b in g9)
    g11 = (a for a in g10 for b in g10)
    g12 = (a for a in g11 for b in g11)
    g13 = (a for a in g12 for b in g12)
    g14 = (a for a in g13 for b in g13)
    g15 = (a for a in g14 for b in g14)
    g16 = (a for a in g15 for b in g15)
    g17 = (a for a in g16 for b in g16)
    g18 = (a for a in g17 for b in g17)
    g19 = (a for a in g18 for b in g18)
    g20 = (a for a in g19 for b in g19)
    g21 = (a for a in g20 for b in g20)
    g22 = (a for a in g21 for b in g21)
    g23 = (a for a in g22 for b in g22)
    g24 = (a for a in g23 for b in g23)
    g25 = (a for a in g24 for b in g24)
    list(g25)
