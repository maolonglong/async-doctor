# m3c_relimport_missing

True-positive: relative import to a module that does not exist. Scanning the
package root must still emit `unresolved-import` for `.missing_mod`.
