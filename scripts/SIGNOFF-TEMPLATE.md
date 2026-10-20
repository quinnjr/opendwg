# opendwg AutoCAD sign-off

Generated {{DATE}} from opendwg commit `{{COMMIT}}` by `scripts/make-signoff.sh`.

This folder holds drawings written by opendwg, a program that writes AutoCAD
drawing (`.dwg`) files without AutoCAD. Please open each one in AutoCAD and
record what you see. Nothing here can harm AutoCAD or your computer: the
worst case is an error message, which is exactly what we want to hear about.

## Before you start

- **What you need:** AutoCAD 2018 or later on Windows (AutoCAD LT works for
  everything except the 3D solid check S3, which needs full AutoCAD). DWG
  TrueView only shows drawings: with it you can do checks 1 and 3 only;
  write `N/A (TrueView)` for the others.
- **The folders:**
  - this folder: the drawings to check, and this file;
  - `originals/`: the drawings some files were made from, for comparison;
  - `untouched/`: spare copies of the drawings, as opendwg wrote them. Do
    not open these. The developer needs them as they are.
- **Recording results:** open this file in Notepad (or print it) and fill in
  the tables. Write `Pass`, `Fail` or `N/A` in the Result column. When
  something fails, write in the Notes column what you saw, word for word if
  there was a message. A screenshot (Windows key + Shift + S, then paste
  into Paint and save it in this folder) is very welcome; mention its file
  name in the notes.
- **A notice you will see and can ignore:** AutoCAD may say the file was
  "saved by a software application that was not developed or licensed by
  Autodesk" (a "non-Autodesk DWG" notice). That is expected. Close it and
  continue; it is not a fail.
- **Command line:** where this file says "type `AUDIT`", click in the command
  line at the bottom of the AutoCAD window, type the word and press Enter.

## The checks for every file

Do these four checks on every file, in order:

{{CHECKS}}

After check 4 the file in this folder has been saved by AutoCAD. Leave it
there: the developer compares it with the copy in `untouched/`.

## The files

{{FILE_TABLE}}

Files that could not be made this time:

{{SKIPPED}}

## Results per file

{{FILE_CHECKS}}

## Specific things to verify in AutoCAD

These are choices opendwg had to make without documentation. Each check
says which file to open, what to do and what to look for. Do them after the
per-file checks (any file will do, saved or not).

### S1. Thumbnail previews

1. In AutoCAD, choose **Open** (Ctrl+O) and browse to this folder; do not
   open anything yet.
2. Click once on `corpus_acadsharp-sample-ac1032.dwg`, then on
   `new_every_entity.dwg`, and look at the Preview box.

Look for: no error message. The sample shows a small picture or an empty
preview box; the new drawing shows an empty preview box. Either is fine;
record which.

| Result (Pass / Fail) | What the preview showed |
|---|---|
| | |

### S2. Identity of a new drawing: dates, IDs and object numbers

opendwg fills in the dates, the two drawing IDs and the internal object
numbers (handles) of a new drawing itself.

1. Open `new_empty.dwg`. Type `DWGPROPS` and open the **Statistics** tab.
   Write down "Created" and "Modified". Close the dialog.
2. Type `FINGERPRINTGUID` and press Enter: AutoCAD prints a long ID. Then
   type `VERSIONGUID` and press Enter. Compare both with the values listed
   under `new_empty.dwg`'s "Expected" in "Results per file" (with or
   without the braces `{ }`); write "same" or what it showed.
3. Open `new_every_entity.dwg`. Click any line and type `LIST`: note the
   "Handle" it shows. Press Esc. Type `LINE`, click two points, press
   Enter. Click the new line and type `LIST`: note its handle. Close
   without saving.

Look for: dates on the day the pack was made ({{DATE}}) or close to it, not
1 January 4713 BC or 1900; the two IDs equal the listed values (not all
zeros); the new line gets a handle that no other object has (AutoCAD
would report a duplicate in `AUDIT`) and is a larger hexadecimal number
than the first line's; no error.

| Result (Pass / Fail) | Created | Modified | IDs as listed? | First line's handle | New line's handle |
|---|---|---|---|---|---|
| | | | | | |

### S3. 3D solids in drawings from older AutoCAD versions

opendwg moves the shape data of 3D solids from the old storage (AutoCAD
2004-2010) into the AutoCAD 2018 storage. Do this for both
`corpus_acadsharp-sample-ac1018.dwg` (from AutoCAD 2004) and
`corpus_acadsharp-sample-ac1024.dwg` (from AutoCAD 2010), and for each
compare with the file of the same name in `originals/`:

1. Open the file. Type `-VIEW`, then `SWISO` (a 3D view from the
   south-west). Type `VSCURRENT`, then `C` (shaded "Conceptual" look).
2. Type `ZOOM`, then `E`. Look at the solids (boxes and other 3D shapes)
   and the flat region.
3. Click one solid, type `MASSPROP` and press Enter. Write down the Volume.
4. Do the same in the original file, on the same solid.

Look for: the solids look the same as in the original, shaded; `MASSPROP`
gives the same volume in both; clicking a solid shows it as "3D Solid" in
the Properties palette (Ctrl+1). In the 2004 file, the solids' "History"
was removed on purpose: "Show History" being off or empty is expected.

| File | Result (Pass / Fail) | Volume here | Volume in original | Notes |
|---|---|---|---|---|
| `corpus_acadsharp-sample-ac1018.dwg` | | | | |
| `corpus_acadsharp-sample-ac1024.dwg` | | | | |
| `edit_renumbered_ac1024.dwg` (compare with `corpus_acadsharp-sample-ac1024.dwg`) | | | | |

`edit_renumbered_ac1024.dwg` is the 2010 drawing with every internal
object number moved by opendwg before saving: its solids must look and
measure exactly like those of `corpus_acadsharp-sample-ac1024.dwg` (do
steps 1-3 in both). Click the same solid in each and type `LIST`: the
handle in the renumbered file is 1000 (hexadecimal) higher; write both.

Also expected, not a fail: the 3D solids and the region of the original
AutoCAD 2000 drawing are missing from `corpus_acadsharp-sample-ac1015.dwg`
on purpose. opendwg cannot yet convert AutoCAD 2000 solid data, so it
removed them before saving.

### S4. Table styles from older AutoCAD versions

Table styles changed format in AutoCAD 2010; opendwg converts older ones.

1. Open `edit_table_style.dwg`. Type `TABLESTYLE`. Select "Standard" and
   click **Modify**.
2. In "Cell styles", pick **Data**, open the **Text** tab: write down the
   text height. Open the **General** tab: write down the horizontal and
   vertical margins. Do the same for **Header** and **Title**.
3. Click OK, then Close. Type `TABLE`, choose 3 columns and 3 data rows,
   click OK and place the table anywhere. Type a word in a data cell.
4. Repeat steps 1-2 (no need to make a table) in
   `corpus_acadsharp-sample-ac1015.dwg`,
   `corpus_acadsharp-sample-ac1018.dwg` and
   `corpus_acadsharp-sample-ac1021.dwg`, and compare with the same file
   in `originals/`.

Look for: in `edit_table_style.dwg`, Data text height 0.25 (the other
values are listed under the file's "Expected" in "Results per file"); the
new table's data text is visibly taller than in the original drawing's
tables. In the other three files, the same values as in the original. No
error message; `AUDIT` (check 2) found nothing about table styles.

Not visible in AutoCAD: the older table style's "flags" value, which
opendwg stores in a field whose meaning is not documented. No dialog
shows it; it is covered by the file opening with no recovery message and
`AUDIT` finding nothing, and the developer compares it in the copy AutoCAD
saved in check 4.

| File | Result (Pass / Fail) | Data / Header / Title text height | Margins | Notes |
|---|---|---|---|---|
| `edit_table_style.dwg` | | | | |
| `corpus_acadsharp-sample-ac1015.dwg` | | | | |
| `corpus_acadsharp-sample-ac1018.dwg` | | | | |
| `corpus_acadsharp-sample-ac1021.dwg` | | | | |

### S5. A dimension with a scale factor

The bottom dimension uses style DIMLFAC2, which multiplies every
measurement by 2 for display. You will draw the same dimension yourself
in AutoCAD and compare it with opendwg's.

1. Open `edit_dimlfac.dwg`. Type `ZOOM`, then `E`. Two horizontal
   dimensions should show; if they do not, type `REGEN`. If they still do
   not show, type `DIMEDIT`, then `H`, select both dimensions (drag a box
   around the drawing) and press Enter.
2. Read the number on each dimension.
3. Click the bottom dimension, open Properties (Ctrl+1) and find
   "Measurement" (in the Text group). Write it down. Press Esc.
4. Make your own dimension over the same points: type `-DIMSTYLE`, then
   `R`, then `DIMLFAC2` (makes that style current). Type `DIMLINEAR`, then
   type exactly `#0,0` Enter, `#6,0` Enter, `#3,-2` Enter (the `#` makes
   AutoCAD read the numbers as exact drawing coordinates). A new
   dimension appears under the other two.
5. Read its number; click it and write down its "Measurement" in
   Properties, as in step 3.

Look for: the bottom dimension (style DIMLFAC2) reads 12, the top one
(style Standard) reads 6, and your own dimension reads the same as the
bottom one. The "Measurement" of opendwg's bottom dimension must equal
that of your own dimension: if they differ, it is a Fail (write both).

| Result (Pass / Fail) | Bottom reads | Top reads | Yours reads | Measurement: bottom | Measurement: yours | Needed step (none / REGEN / DIMEDIT) |
|---|---|---|---|---|---|---|
| | | | | | | |

### S6. Dimensions made without a picture

opendwg makes new dimensions without the picture AutoCAD keeps for each.

1. Open `new_every_entity.dwg` and type `ZOOM`, then `E`.
2. Look for the dimensions around the lower left corner (they overlap
   other objects): straight, aligned, angle,
   radius, diameter, ordinate, arc length (with an arc symbol) and a
   jogged radius.
3. Click the arc length dimension and type `LIST`; then the jogged one and
   `LIST`. Note the name AutoCAD gives each (first line of the list).

Look for: whether the dimensions show as soon as the file opens, or only
after `REGEN`, or only after `DIMEDIT` / `H` (as in S5). No error message.

| Result (Pass / Fail) | Visible when (opening / REGEN / DIMEDIT / never) | Names LIST gave |
|---|---|---|
| | | |

### S7. What every new drawing contains

opendwg builds new drawings itself, without AutoCAD's template. Open
`new_empty.dwg` and try each command below. Each should open normally and
show what is listed.

| Command | Should show | Result (Pass / Fail) | Notes |
|---|---|---|---|
| `LAYER` | one layer, "0" | | |
| `STYLE` | text styles "Standard" and "Annotative" | | |
| `DIMSTYLE` | dimension styles "Standard" and "Annotative" | | |
| `MLEADERSTYLE` | "Standard" and "Annotative" | | |
| `TABLESTYLE` | "Standard" | | |
| `VISUALSTYLES` | a palette of visual styles (2D Wireframe, Conceptual, Realistic, ...); double-click "Conceptual": the view turns shaded | | |
| `MATBROWSEROPEN` | the Materials Browser; "Document Materials" lists "Global" | | |
| `VIEWSECTIONSTYLE` | "Imperial24" | | |
| `VIEWDETAILSTYLE` | "Imperial24" | | |
| `SCALELISTEDIT` | a list of scales (1:1, 1:2, ... ) | | |
| `PAGESETUP` | the Page Setup Manager, no error | | |
| Layout tab | a "Layout1" tab next to "Model"; clicking it shows a sheet | | |

Still in `new_empty.dwg`, type each of these names and press Enter;
AutoCAD prints the current value (press Esc if it asks for a new one).
These are the drawing settings opendwg writes into a new drawing.

| Type | Should show | Shows | Result (Pass / Fail) |
|---|---|---|---|
| `DIMASSOC` | 2 | | |
| `HIDETEXT` | 1 (On) | | |
| `CTABLESTYLE` | Standard | | |
| `CMLEADERSTYLE` | Standard | | |
| `CANNOSCALE` | 1:1 | | |
| `LAYEREVAL` | 0 | | |
| `LAYERNOTIFY` | 0 | | |
| `CVIEWSECTIONSTYLE` | Imperial24 | | |
| `CVIEWDETAILSTYLE` | Imperial24 | | |

Also record how many entries `SCALELISTEDIT` lists (opendwg writes 33
scales) and how many visual styles `VISUALSTYLES` shows (AutoCAD hides some
of the 24 opendwg writes; just write the number): Scales ______
Visual styles ______

Then open `corpus_acadsharp-sample-ac1021.dwg`,
`corpus_acadsharp-sample-ac1024.dwg` and
`corpus_acadsharp-sample-ac1027.dwg` and try `VISUALSTYLES` (double-click
"Realistic") and `VIEWSECTIONSTYLE` in each.

| File | Result (Pass / Fail) | Notes |
|---|---|---|
| `corpus_acadsharp-sample-ac1021.dwg` | | |
| `corpus_acadsharp-sample-ac1024.dwg` | | |
| `corpus_acadsharp-sample-ac1027.dwg` | | |

### S8. A large drawing

1. Open `new_200k_lines.dwg` and time roughly how long it takes to open.
2. Type `ZOOM`, then `E`: a long band of short lines shows.

| Result (Pass / Fail) | Seconds to open | Notes |
|---|---|---|
| | | |

### S9. Things AutoCAD does not show

Some choices sit in parts of the file that no AutoCAD command shows: the
undocumented numbers in the file's opening header and its "auxiliary"
header (version strings, maintenance version, unknown values), the bits
that pad the end of each object, how some internal sections are laid out,
the order of objects and the list of object classes. There is nothing to
look at for these. They are covered by the per-file checks: a file that
opens with no recovery message, passes `AUDIT` with 0 errors and saves
and reopens is accepted. If any file fails check 1, 2 or 4, that is the
result the developer needs. The copies AutoCAD saved in check 4 let the
developer compare these values with what AutoCAD writes.

Nothing to record here.

## When you are done

Signed off by: ______________________  Date: ____________

AutoCAD product and version (Help > About): ______________________

Then zip this whole folder (right-click it, **Send to** > **Compressed (zipped)
folder**) and send the zip to the developer. It includes this file with your
results, the drawings AutoCAD saved in check 4, any screenshots, and the
`untouched/` copies.

---

## For the developer: what each check covers

Unverified decisions behind these checks (from Tasks 11-25):

| Decision | Covered by |
|---|---|
| File header and AuxHeader values not documented by ODS (undocumented longs, version strings, unknown pairs, maintenance version 0), writer end padding bits, section paging of VBAProject/FileDepList/Security/Signature | not visible in AutoCAD (S9): checks 1, 2 and 4 on every file; compare the AutoCAD-saved copies |
| Preview written at the R2018 preview address | S1 |
| New drawings fill handles, creation/update dates and GUIDs; non-AutoCAD handle numbers; $HANDSEED above every handle | S2 (DWGPROPS dates, FINGERPRINTGUID/VERSIONGUID against the written values, LIST handles of an existing and a new line), checks 1-2 on `new_*.dwg` |
| ACIS from R2004-R2010 files written into the R2018 AcDs section (fresh AcDs layout and schema order, deterministic GUIDs, empty ACIS without 137 trailing bits); R2004 solid history and R2000 SAT unconvertible | S3, checks 1-4 on the converted corpus files |
| AcDs records follow a handle renumbering (`rewrite_handles`) | S3 row `edit_renumbered_ac1024.dwg` (and `tests/signoff.rs`: same ACIS bytes under the shifted handles) |
| Pre-R2010 TABLESTYLE converted with R2010 "Standard" defaults; "Table" cell style height and margin spacing taken from the data row | S4 |
| DXF 71 flags in `unknown_bl[0]` | not visible in AutoCAD (S4 note): checks 1, 2, 4 on the converted files; compare the AutoCAD-saved copy |
| Recomputed actual measurement (DXF 42) not scaled by DIMLFAC | S5: Measurement of opendwg's dimension against one AutoCAD draws over the same points in the same style (and the AutoCAD-saved `edit_dimlfac.dwg`) |
| Dimensions written without a `*D` block; ARC_DIMENSION and LARGE_RADIAL_DIMENSION class definitions; the 17-class list of new drawings | S6, checks 1-2 on `new_every_entity.dwg` |
| New-drawing object set (every template object table): no FBXASSET, materials, visual styles, section/detail view styles, scale list (33), PLOTSETTINGS class, layouts, the variable dictionary values | S7 (managers, counts, and the variable table) |
| Pre-R2018 VISUALSTYLE copied as marked; section/detail view styles missing the BS 70 field before R2018 | S7 (corpus files) |
| Insert lists, class counts, object order kept as read; R2000 VP_ENT_HDR table dropped | not visible as such (S9): checks 2-4 on the corpus files |
| Re-encoding with canonical (shortest) forms where AutoCAD used others (pattern (c), 2% tolerance) | checks 2-4 on the corpus files |
| Near-linear writing of large drawings | S8 |
| ODA File Converter local opt-in only (`tests/oda.rs`); CI relies on the validator and this manual check | this whole checklist |
