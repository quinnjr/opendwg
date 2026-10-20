# Sample corpus

Public DWG samples used by the read/write tests. The files are not stored in this repository.

    scripts/fetch-corpus.sh        # downloads to corpus/files/, verifies SHA-256, idempotent

`manifest.toml` lists each file: `name`, `url` (pinned to a commit), `sha256`, `version` (file magic),
`features` (tags: hatch, dimension, attributes, mtext, image, xref, custom_objects, proxy_graphics,
non_ascii), `license_note`, and an `expect` table (filled in by later tasks).
Only files under permissive terms are listed. Never add files from GPL projects (e.g. LibreDWG).

Tests load the manifest through `tests/common/mod.rs`; tests that need the files skip when they
have not been fetched.

Known gaps: no `xref` file, no Civil 3D drawing, no R2000/R2004 file with custom objects, and no
R2000/R2004 file with Shift-JIS or GBK text (no public file under acceptable terms found).
