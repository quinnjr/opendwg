# Sample corpus

Public DWG samples used by the read/write tests. The files are not stored in this repository.

    scripts/fetch-corpus.sh        # downloads to corpus/files/, verifies SHA-256, idempotent

`manifest.toml` lists each file: `name`, `url` (pinned to a commit), `sha256`, `version` (file magic),
`features` (tags: hatch, dimension, attributes, mtext, image, xref, custom_objects, proxy_graphics,
non_ascii), `license_note`, and an `expect` table (filled in by later tasks).
Only files under permissive terms are listed. Never add files from GPL projects (e.g. LibreDWG).

Tests load the manifest through `tests/common/mod.rs`; tests that need the files skip when they
have not been fetched.

## Extended corpus

`extended.toml` lists 134 more real-world files (same schema, plus `producer`, and an optional
`expect.open = "unsupported"` or `expect.unconvertible = true`), fetched by the same script:

- Civil 3D 2020-2025 drawings with AECC objects (`camber-*`, `c3dtk-01`, `rc-03`);
- xrefs: `cc-12` (R2000, unloaded xref block "Level 1" to `..\plans\Level 1.dwg`, target `cc-13`)
  and `camber-26` (Civil 3D xref host of `camber-24`, `Xref01|...` dependent layers);
- GBK (ANSI_936) text: `dsh-01/02` (SolidWorks R2000), `wz-01/02` (R2000), `fox-01/02` (R2013);
- R2000/R2004 files with custom objects (`aspj-01`, `gdal-03`..`07`, `cc-*` R2000 fixtures);
- other writers: BricsCAD (`brx-*`, `ezdxf-01..03`), ODA SDK across R2004-R2018 (`cc-15..25`),
  Teigha (`geo7-01`, `asp-15`), acadrust (`codecad-*`), civilcraft's own writer (`cc-06/08/09/12/13`);
- large drawings (`rc-01`, 260 000 objects; `adsk-house-01`), Autodesk's own samples (`adsk-*`,
  `arx-*`), ACadSharp's newer samples (dynamic block parameters, geolocation, `sample_base`);
- negative cases: R14 (`arx-01`, `acadsharp-sample-ac1014`), R13 (`arx-11`), the pre-release
  "AC402b" (`arx-04`) and R12/R10 (`usfs-01/02`).

Feature tags were computed with the library from the decoded drawings (objects of each type, xref
block records, class-numbered raw objects, entities with proxy graphics, non-ASCII strings).
`tests/extended_corpus.rs` runs every file through open, validate and save.

Left out on licence grounds (third-party content under a repository licence only): GDAL's Mapwize
building, Aspose's copies of Autodesk samples, a file a user attached to an ezdxf issue, and an
architectural plan (a DXF file named `.dwg`) whose repository licence is a site template's.

Remaining gaps: no Shift-JIS, Big5 or Korean file; no R2000 or R2004 Civil 3D file; no Map 3D,
Plant 3D or MEP objects (no public file under acceptable terms found).
