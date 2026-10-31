# m3c_relimport_pkg_root

Scan root **is** the package. Relative imports whose targets exist must not
become `unresolved-import` FPs (ty often fails `resolve_module` in this layout).
Includes a PEP 420 namespace package (`ns_pkg/` without `__init__.py`).

Expect: **0** unresolved-import records with `--show-unresolved`.
