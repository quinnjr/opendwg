# Third-party code

## ACadSharp (MIT) - https://github.com/DomCR/ACadSharp
Consulted (not copied) to verify format details that ODS leaves unclear. Checked in
`src/ACadSharp/IO/DWG/DwgStreamReaders/DwgStreamReaderAC18.cs`:
- CMC (R2004+) color-method top byte of the RGB long: 0xC0000000 = ByLayer, bit 0x01000000 =
  ACI index in the low byte, otherwise true color. Used in `src/bits/color.rs`.
- ENC 0x8000 true-color value is read as a BL (not a BS); 0x4000 means a color-book handle follows
  and no RGB value is stored, even when 0x8000 is also set (confirmed on `sample_AC1032.dwg`).
Used in `src/bits/color.rs` and mirrored by the writer in `src/bits/writer.rs`.

## ACadSharp (MIT) - objects, code pages
Consulted (not copied) `src/ACadSharp/IO/DWG/DwgStreamReaders/DwgObjectReader.cs`,
`DwgStreamReaderBase.cs` and `src/ACadSharp/CadUtils.cs` where ODS chapters 3, 20 and 28 are
silent or wrong. Every point below was checked against the corpus samples (all versions): every
object splits with a valid CRC, every entity's layer resolves to a LAYER, LINE bodies end exactly
at the data stream end. Used in `src/strings.rs` and `src/object/`:
- DWG code page index table (ODS prints none): `CadUtils._pageCodes`.
- Object stream positions: sizes count from after the `MS` (R2000-R2007) or the `MC` (R2010+);
  the R2010+ handle stream size `MC` is unsigned; the string stream size is in bits. ODS 20.1
  says the end bit moves back "by 16 bytes" per size short: it is 16 bits (one `RS`).
- Entity common data: no `nolinks` bit in R2004+ (ODS says it is always 1); material flags and
  shadow byte come before the plot style flags (ODS lists plot style first); no owner handle
  unless entmode is 0.
- Extended data: pre-R2007 strings are `RS` length then `RC` code page (ODS says `RC` then `RS`);
  1003/1005 handles are 8 bytes big-endian.

## ACadSharp (MIT) - symbol tables
Consulted (not copied) `readXrefDependantBit`, `readLayer`, `readTextStyle`, `readLType`,
`readView`, `readVPort`, `readDimStyle` and `readBlockHeader` in
`src/ACadSharp/IO/DWG/DwgStreamReaders/DwgObjectReader.cs` where ODS 20.4.51-20.4.70 is wrong or
silent. Every point was checked on the corpus (each table object decodes to its last bit and R2018
objects re-encode byte for byte). Used in `src/object/tables.rs`:
- R2007+ table entries store one `BS` for the xref fields, with bit 0x100 for xref-dependent (ODS:
  `B` 64-flag, `BS` xrefindex+1, `B` xdep).
- LAYER: the trailing unknown handle is R2013+ only; flags bit 2 means off.
- STYLE: the shape-file bit precedes the vertical bit.
- DIMSTYLE: DIMCLRD/DIMCLRE/DIMCLRT are `CMC`.
- LTYPE: the R2007+ text area is present when a dash has the text flag (0x02).
- The lineweight index table (`CadUtils._indexedValue`) to read layer lineweights 9 (0.35 mm), 11
  (0.50 mm) and 31 (Default) in `tests/tables.rs`.
- BLOCK_HEADER: from R2004 an xref or overlaid xref block stores no owned object count (and no
  owned entities); ODS has the count always. Confirmed on Civil 3D's xref block in `camber-26` of
  the extended corpus (`corpus/extended.toml`), which decodes to its last bit only so.

## ACadSharp (MIT) - basic, text and insert entities
Consulted (not copied) `readInsertCommonData`, `readInsertCommonHandles`, `readCommonAttData`,
`readEntityMode` and `readMText` in `src/ACadSharp/IO/DWG/DwgStreamReaders/DwgObjectReader.cs`
where ODS 20.4.4, 20.4.9 and 20.4.46 are wrong or silent. Every point was checked on the corpus
(each entity decodes to its last bit and R2018 entities re-encode byte for byte) and is pinned by a
value test in `tests/entities.rs`. Used in `src/object/entities/`:
- INSERT/MINSERT: the R2004+ owned-object count is stored only when the insert has attributes
  (ODS: always).
- MTEXT: the background scale factor is a `BD` (ODS: `BL`).
- A multi-line R2018 ATTRIB/ATTDEF embeds an MTEXT whose common data starts at the entity mode
  and runs through the whole R2000+ entity common data (layer, linetype, colors, ...), then the
  MTEXT fields, then the annotative data size.
Found on the corpus, in neither source: the R2018 MTEXT context version is 4 (ODS: default 0),
entity owners are often stored as offsets from the entity's handle, and a true color's ENC word
keeps its nearest ACI index with the RGB long carrying method byte 0xC2.

## ACadSharp (MIT) - polylines, splines and mlines
Consulted (not copied) `readLWPolyline` and `readSpline` in
`src/ACadSharp/IO/DWG/DwgStreamReaders/DwgObjectReader.cs`, and `writePolygonMesh` in
`src/ACadSharp/IO/DWG/DwgStreamWriters/DwgObjectWriter.Entities.cs`, where ODS 20.4.34, 20.4.40 and
20.4.85 are wrong or silent. Every point the corpus has was checked on it (each entity decodes to
its last bit and R2018 entities re-encode byte for byte) and is pinned by a value test in
`tests/polylines.rs`. Used in `src/object/entities/poly.rs`:
- LWPOLYLINE flag bits 0x100 and 0x200 are "plinegen" and "closed" (ODS lists only the bits of the
  stored fields).
- An R2013+ SPLINE stores fit data exactly when its knot parameter is not "custom" (15) and flags
  1 has "use knot parameter" (8); the scenario field does not decide (the corpus has fit data with
  scenario 1).
- R2004+ polygon mesh vertices are written as hard owners (ODS: soft pointers; not in the corpus).
Found on the corpus, in neither source: R2004+ polyface mesh vertices are hard owners (ODS: soft
pointers; ACadSharp writes soft pointers), and the MLINE "open/closed" short is DXF 71's flags (5
occurs).

## ACadSharp (MIT) - dimensions, leaders and multileaders
Consulted (not copied) `readCommonDimensionData`, `readLeader`, `readTolerance`, `readMultiLeader`,
`readMultiLeaderAnnotContext`, `readLeaderRoot`, `readLeaderLine` and `readBreakInfo` in
`src/ACadSharp/IO/DWG/DwgStreamReaders/DwgObjectReader.cs`, and `writeMultiLeader` and its context
writer in `src/ACadSharp/IO/DWG/DwgStreamWriters/DwgObjectWriter.Entities.cs`, where ODS 20.4.47,
20.4.48, 20.4.49 and 20.4.86 are wrong or silent. Every point the corpus has was checked on it (each
entity decodes to its last bit and R2018 entities re-encode byte for byte) and is pinned by a value
test in `tests/dimensions.rs`. Used in `src/object/entities/dim.rs` and `src/object/entities/mleader.rs`:
- From R2010 a LEADER stores no text box height and width (ODS: all versions).
- TOLERANCE text is a `TV` (ODS: `BS`).
- The MULTILEADER arrowhead list is stored up to R2007 inclusive (ODS: "-R2007"), its `BS 175` is
  the text alignment type (ODS: unknown), and a leader line's break info repeats (ODS shows one).
Found on the corpus, in neither source: DIMENSION "Flags 1" has bit 3 set on every dimension (ODS:
only bits 0 and 1 are used), and a LEADER with a hook line stores one point fewer than its DXF export
lists. Not in the corpus, so following ODS: a block label's ATTDEF is a soft pointer (ACadSharp
writes a hard pointer).

## ACadSharp (MIT) - hatches, viewports, images and ACIS entities
Consulted (not copied) `readHatch`, `readViewport`, `readCadImage`, `readModelerGeometry`,
`readModelerGeometryData`, `readSolid3D`, `readOle2Frame` and `readCommonProxyData` in
`src/ACadSharp/IO/DWG/DwgStreamReaders/DwgObjectReader.cs`, `writeViewport` in
`src/ACadSharp/IO/DWG/DwgStreamWriters/DwgObjectWriter.Entities.cs`, and `src/ACadSharp/IO/AcisTextCodec.cs`,
where ODS 20.4.38, 20.4.41, 20.4.75, 20.4.80, 20.4.88 and 20.4.90 are wrong or silent. Every point
the corpus has was checked on it (each entity decodes to its last bit and R2018 entities re-encode
byte for byte) and is pinned by a value test in `tests/hatch_misc.rs`. Used in
`src/object/entities/hatch.rs` and `src/object/entities/misc.rs`:
- A HATCH gradient color is a value and a `CMC` (ODS: a BD, an unknown BS, a BL and an ignored RC).
- A VIEWPORT's clip boundary is written as a hard pointer (ODS: soft).
- A WIPEOUT is laid out as an IMAGE (ODS does not describe it).
- From R2013 an ACIS entity has no empty bit, version or modeler data (its SAB is in the AcDs
  section); REGIONs store no R2007+ history handle (ODS: all ACIS entities).
- A version 2 ACIS payload ends at its end-of-data marker, spelled as SAB tagged strings; version 1
  SAT blocks decrypt by mapping 0x21-0x7E to 0x9F minus the byte (ODS: 0x20-0x7E).
- A proxy's pre-R2018 drawing format holds the version in its low 16 bits.
Found on the corpus, in neither source: R2013+ ACIS entities end, after the R2007 unknown `BL`,
with a `B` (DXF 290), the entity's GUID (DXF 2: `BL`, `BS`, `BS`, 8 `RC`) and a 0 read as a `BS`,
checked against the DXF export. A hatch spline edge stores its R2010+ start and end tangents only
after a non-zero fit point count, as ACadSharp reads them (ODS: always); confirmed on AutoCAD
2025's hatch in `acadsharp-svg-export-sample` of the extended corpus. Not in the corpus, so
following ODS: ACIS silhouettes (a `BL` viewport id and a `BL` wire count; ACadSharp reads a `BLL`
and a flag), the OLE2FRAME and ACAD_PROXY_ENTITY layouts (ACadSharp reads an extra text from R2004
in proxies).

## ACadSharp (MIT) - non-graphical objects and informational sections
Consulted (not copied), at commit `ed5ff1cf4731c0296bb8931a387bd9d7c634122f`:
`readCommonDictionary`, `readDictionaryWithDefault`, `readDictionaryVar`, `readXRecord`,
`readLayout`, `readPlotSettings`, `readGroup`, `readMLineStyle`, `readMultiLeaderStyle`,
`readTableStyle`, `readRowCellStyle`, `readBorderStyle`, `readImageDefinition`,
`readImageDefinitionReactor`, `readRasterVariables`, `readScale`, `readSortentsTable`,
`readSpatialFilter` and `readProxyObject` in `src/ACadSharp/IO/DWG/DwgStreamReaders/DwgObjectReader.cs`;
`readCellStyle`, `readCellContentFormat` and `readBorder` in `DwgObjectReader.Entities.cs`;
`readWipeoutVariables` in `DwgObjectReader.Objects.cs`; `src/ACadSharp/GroupCodeValue.cs`;
`src/ACadSharp/IO/Templates/CadMLineStyleTemplate.ElementTemplate.cs`; `DwgSummaryInfoReader.cs`,
`DwgPreviewReader.cs` and `DwgAppInfoReader.cs` in `src/ACadSharp/IO/DWG/DwgStreamReaders/`; and
`DwgAppInfodWriter.cs` and `DwgPreviewWriter.cs` in `src/ACadSharp/IO/DWG/DwgStreamWriters/`,
where ODS 13, 14, 16 and 20.4.44-104 are wrong or silent. Every point the corpus has was checked
on it (each object decodes to its last bit, R2018 objects and sections re-encode byte for byte)
and is pinned by a value test in `tests/objects.rs` or `tests/sections.rs`. Used in
`src/object/objects.rs`, `src/section/misc.rs` and `src/drawing/mod.rs`:
- XRECORD item types by DXF group code range (`GroupCodeValue.TransformValue`); before R2007
  their text is an `RS` length and an `RC` code page (ODS: an `RC` length).
- A LAYOUT's viewport count is a `BL` (ODS: `RL`).
- MLEADERSTYLE: the text angle type and align-left fields are read in every version, and R2013
  adds a `B` (DXF 298).
- The R2010+ TABLESTYLE cell styles, content formats and borders, and the unknown index before
  each cell style.
- WIPEOUTVARIABLES (not in ODS) is one `BS`.
- A pre-R2018 MLINESTYLE linetype index of 32767 is BYLAYER and 32766 BYBLOCK.
- SummaryInfo strings are in the drawing's code page before R2007.
- The preview's PNG entry has code 6.
- The R21 AppInfo layout's second `UInt32` counts the checksummed strings ("num strings"), and
  opendwg's own AppInfo follows ACadSharp's writer (class version 3, then version, comment and
  product strings with zero checksums).
Found on the corpus, in neither source: the R2010 form AutoCAD gives a pre-R2010 TABLESTYLE
(`LegacyTableStyle::to_r2010`, checked against the R2010 sample); dictionary entries are soft
owners even in hard-owner dictionaries; MLEADERSTYLE stores the annotative bit before the
property-changed bit (ODS: after); a TABLESTYLE in an R2000 file has R2004 colors; the CMC color
method 0xC8 (no color); R2000 layouts' viewports are the VIEWPORT entities of their block. Not in
the corpus, so following ODS: PLOTSETTINGS, ACAD_PROXY_OBJECT, LAYER_INDEX and the R18 AppInfo
layout.

Proxy graphics (`src/proxy_graphics.rs`) follow ODS chapter 29 only; no ACadSharp code was
consulted. Found on the corpus and absent from ODS (pinned in `tests/proxy.rs`): a true color
primitive is one packed `RL` (a CMC color word: method in the top byte, 0xC0 ByLayer), not three
`RC`; opcodes 21 (empty) and 51 (one `RL`, 0 or 0x01000000) occur and are kept as unknown; ODS
gives "Unicode text 2" no opcode number, and 38 is the number the corpus uses; AEC entities use
the Unicode text opcode 36 and tables and multileaders from R2007 use 38; text padding bytes are
not zero; shell face counts are positive.

## ACadSharp (MIT) - materials and visual styles
Consulted (not copied), at commit `ed5ff1cf4731c0296bb8931a387bd9d7c634122f`, `readMaterial` and
`readVisualStyle` in `src/ACadSharp/IO/DWG/DwgStreamReaders/DwgObjectReader.cs`, where ODS has no
MATERIAL or VISUALSTYLE. Used in `src/object/objects/render.rs`: the MATERIAL field order (the
channel colors, maps with their 4x4 matrices and sources, and the R2007+ tail), checked on every
R2007+ corpus material (each decodes to its last bit; R2018 ones re-encode bit for bit). ACadSharp
reads only the start of a VISUALSTYLE (name, type, a `BS`, a `B`); the rest of its layout, and the
ACDBSECTIONVIEWSTYLE and ACDBDETAILVIEWSTYLE layouts, were found on the corpus with the samples'
DXF exports and are pinned in `tests/render_styles.rs`.

## ezdxf (MIT) - ACI palette
https://github.com/mozman/ezdxf, tag `v1.3.0`, `src/ezdxf/colors.py` `DXF_DEFAULT_COLORS`,
Copyright (c) 2020-2023 Manfred Moitzi, MIT licence. `scripts/gen-aci-palette.py` reads it and generates
the table `ACI_RGB` in `src/bits/color.rs`, used to compute the nearest ACI index of an edited true
color. 26 entries that depart from the palette's hue/shade/gray structure (digit slips such as 88
for 38 in dark shades, and an older gray ramp) are replaced by the structural value; the unit test
`aci_palette_structure` checks every entry.

## ACadSharp (MIT) - R2004 LZ77
Consulted (not copied) `src/ACadSharp/IO/DWG/DwgLZ77AC18Decompressor.cs` where ODS section 4.7 is
ambiguous. Used in `src/codec/lz77_2004.rs`:
- Opcodes `0x10`-`0x1F`: low 3 bits give the length (0 = long form, +9; else +2) and bit `0x08`
  adds `0x4000` to the distance (ODS writes `(opcode & 0x0F) + 2`, wrong for `0x18`-`0x1F`).
- All distances are `compOffset + 1`.

## ACadSharp (MIT) - R2007 LZ77 and Reed-Solomon layout
Consulted (not copied) `src/ACadSharp/IO/DWG/DwgStreamReaders/DwgLZ77AC21Decompressor.cs` and the
AC21 reading in `src/ACadSharp/IO/DwgReader.cs`, to confirm the reading of ODS sections 5.10 and
5.13. Used in `src/codec/lz77_2007.rs` and `src/codec/rs2007.rs`:
- The literal copy order (ODS 5.10.1 table) and `ReadInstructions` / chunk loop agree with ODS.
- Interleaved RS blocks: byte `j` of codeword `i` is at `j * block_count + i`, data bytes first.
  ACadSharp only de-interleaves; error correction here is our own (Berlekamp-Massey, Chien,
  Forney) using the ODS 5.13 field polynomials.
- `sample_AC1021.dwg` from the ACadSharp repository was used locally (not committed) to confirm
  the field polynomials, generator roots and byte order.
- First opcode with high nibble 2: `DwgLZ77AC21Decompressor.Decompress` skips 3 bytes after the
  opcode and reads the length from the byte before the new position, i.e. `byte[3] & 7` (ODS
  5.10 wording allows `byte[2]`). Used in `src/codec/lz77_2007.rs`.

## ACadSharp (MIT) - R2007 container
Consulted (not copied) `readFileHeaderAC21`, `getPageBuffer` and `getSectionBuffer21` in
`src/ACadSharp/IO/DwgReader.cs` where ODS chapter 5 is silent on reading. Used in
`src/container/r2007.rs`:
- System pages: the RS block count is `ceil(align8(compressed size) * CorrectionFactor / 239)`.
- Data pages of encoding 4: `ceil(align8(compressed size) / 251)` interleaved blocks; encoding 1
  pages are read as is, without RS. A page is decompressed only when its compressed size differs
  from its decompressed size.
ACadSharp checks no CRCs; the CRC and checksum seeds and byte order used by the reader come from
ODS 5.4.1 and 5.12 and were confirmed on the corpus file.

## ACadSharp (MIT) - header variables
Consulted (not copied) `src/ACadSharp/IO/DWG/DwgStreamReaders/DwgHeaderReader.cs` and
`DwgStreamWriters/DwgHeaderWriter.cs` where ODS chapter 9 is silent or contradicted by the corpus.
Every point was checked on all corpus versions (the header is consumed to the last bit). Used in
`src/section/header.rs` and `src/bits/{reader,writer}.rs`:
- The R2007+ header size in bits counts from the start of its own `RL`; handles follow it.
- `BLL` has a plain 3-bit byte count (ODS 2.4 says 1-3 bits like a 3B).
- `MENUNAME` is written for every version (ODS: R13-R2004 only).
- After the four trailing "type 5/6" `BS` (ODS: R14+), R2004+ headers end with `BL`, `BL`, `B`
  (not in ODS); from R2007 these seven fields follow the handle stream.

## ACadSharp (MIT) - test fixtures (test-only, not published)
The library embeds no data copied from sample drawings. The test fixtures below are copied from
the ACadSharp sample drawings (commit `ed5ff1cf4731c0296bb8931a387bd9d7c634122f`), Copyright (c)
2021 Albert Domenech, MIT licence (reproduced under "ACadSharp licence" below). They are read only
by `#[cfg(test)]` code and are excluded from the published crate (`exclude` in `Cargo.toml`);
`build.rs` sets the `opendwg_fixtures` cfg only when they are present, so the published crate's
tests compile and run without them:
- `src/object/testdata/*.bin`: six objects (35-90 bytes each, `MS` through CRC) from
  `sample_AC1015.dwg`, `sample_AC1018.dwg`, `sample_AC1021.dwg`, `sample_AC1027.dwg` and
  `sample_AC1032.dwg`, for the golden tests in `src/object/common.rs` (`tests::golden`);
- `src/section/testdata/header_ac1015.bin`, `header_ac1018.bin` and `header_ac1021.bin` (575-848
  bytes each): the `AcDb:Header` sections of `sample_AC1015.dwg`, `sample_AC1018.dwg` and
  `sample_AC1021.dwg`, for the hermetic tests in `src/section/header.rs`.

## ACadSharp (MIT) - new drawings
`Drawing::new` (`src/drawing/new.rs`) builds its objects in code from typed values. Which objects
AutoCAD's default template has, how they link, and the value choices (names, colors, sizes, scale
ratios, visual style properties, ...) follow the template objects of the corpus file
`acadsharp-dynblock-lookup` (ACadSharp's `samples/dynamic-blocks/BLOCKLOOKUPPARAMETER.dwg`), and
are written out as named constants and literals; no bytes are copied. `tests/from_scratch.rs`
(`new_matches_the_corpus_template`) compares them with the corpus file when it is fetched. The class
table of a new drawing is that file's first 17 classes, and the class definitions in
`src/section/classes.rs` (`KNOWN_CLASSES`) are those of `sample_AC1032.dwg` from the same commit.

## ACadSharp (MIT) - sample drawings
The public test corpus (`corpus/manifest.toml`) lists DWG files from the ACadSharp repository,
pinned to commit `ed5ff1cf4731c0296bb8931a387bd9d7c634122f`. They are downloaded on demand by
`scripts/fetch-corpus.sh` and are never committed or distributed with this crate. ACadSharp is
Copyright (c) 2021 Albert Domenech, MIT licence (`LICENSE` at the repository root).

## ACadSharp licence
The licence of ACadSharp, which covers the test fixtures above (`src/object/testdata/*.bin` and
`src/section/testdata/header_ac10xx.bin`, not published with the crate), as in
the `LICENSE` file at the root of https://github.com/DomCR/ACadSharp at commit
`ed5ff1cf4731c0296bb8931a387bd9d7c634122f`:

```text
MIT License

Copyright (c) 2021 Albert Domenech

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## Tooling (not distributed)

### ODA File Converter (Open Design Alliance)
Not bundled or redistributed. Considered for Task 26 (round-trip verification by an independent
reader). Checked 2026-10-18.
- The download page (https://www.opendesign.com/guestfiles/oda_file_converter) names no licence.
  The only ODA agreement for free downloads is the *Community User Agreement* (5 Sep 2025,
  https://www.opendesign.com/agreements/2025/en/ODA%20Community%20User%20Agreement%2009-2025.pdf),
  which covers any "free end-user software application made available by the ODA ... including
  ... other free stand-alone utilities". That the File Converter falls under it is our inference;
  the agreement does not name it.
- Scope of use (s3.1.1): "Community User may use each Community Application for its intended
  functionality ... (including, but not limited to, viewing, exploring, and converting supported
  file formats) but may not directly or indirectly resell or distribute a Community Application,
  or any part thereof, as part of any commercial application."
- Automation is neither permitted nor prohibited expressly. The converter's own documented
  feature is a command-line batch mode, so running it from a local test script on a developer
  machine is within "intended functionality".
- Limits that matter for CI: the licence is "non-transferable, non-sublicensable, revocable", and
  "the rights ... may not be transferred or licensed to any other person or entity, including
  Community User's employer, contractor, ...". Each person must accept the agreement themselves.
  A shared CI runner, or a runner that downloads the converter on behalf of contributors, is
  not clearly covered; Documentation may only be used "for evaluation and reference".
- Outcome: running it manually or from an opt-in local test (developer has accepted the agreement
  and installed it; test skipped when absent) is permitted. Running it in hosted/shared CI is not
  clearly permitted, so CI must not download or run it. Task 26 uses the fallback described in the
  spec for CI. Ask ODA (support@opendesign.com) for written confirmation if CI use is wanted.
- Use: `tests/oda.rs` runs a converter the developer installed (`ODA_FILE_CONVERTER` or `PATH`),
  skips with a printed message when it is absent, and never runs it when `CI` is set.
