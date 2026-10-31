# m3c_nested_pkg_flat

Baseline for nested-package recall: same `libcore.cfg` retry (`requests.get` +
`time.sleep`) under a flat `libcore/` layout importable from the scan root.

Expect **4 findings** (2 async entries × blocking-http + blocking-sleep).
