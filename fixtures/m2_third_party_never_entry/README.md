# m2_third_party_never_entry

site-packages `libz.api.fetch` is async and blocks. Findings must attribute
to first-party `handler`, never treat the third-party async def as an entry.
