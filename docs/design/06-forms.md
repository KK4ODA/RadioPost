# RadioPost: forms for most jobs (a proposal)

| | |
|---|---|
| **Status** | Accepted, in progress |
| **Version** | 0.3.0 |
| **Updated** | 2026-11-24 |
| **Editor** | KK4ODA |
| **Licence** | [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/) |
| **Follows** | [The wishlist](02-wishlist.md), W32, W33 and W55; [RP-2](../../spec/rp-0002-messages.md) §8 |

Emergency groups and the agencies they serve work with dozens of forms: messages, resource
requests, radio plans, logs, status grids, weather and damage reports, position reports. RadioPost
has one, ICS-213. This document says how RadioPost can carry most of the forms groups use,
including those that carry a position, without writing program code for each one. The author
accepted it on 2026-11-24 with the answers in §9. Steps 1 to 3 (§8) are built.

**How facts are marked.** **[P]**: read on a primary source. **[S]**: found only secondhand, in a
search result's summary. **[T]**: measured here. Facts about RadioPost were read in this repository
on 2026-11-24.

---

## 1. What groups use

Winlink Express is where most groups' forms live today, so its library shows what is asked for.
Its standard templates are updated over the Internet, and other forms are grouped by state and by
organisation **[S]** (Winlink's forms pages; winlink.org refused automated reading). Some of its
forms carry a position ("mapping enabled"). Such a form opens with the GPS's position, else the
location saved for position reports, else the grid square's centre, and the operator may type
another. At the incident centre the program stores each report's data as the message downloads,
draws status markers on a map, filters them by age and writes a CSV file **[P]** (Winlink's
*Mapping Enabled Winlink Forms*).

RadioPost does not read or write Winlink's forms (W32, the author's decision). It takes the jobs
those forms do, and the public originals most of them come from.

---

## 2. The shapes forms come in

The library is large, but nearly every form has one of five shapes.

| Shape | Examples | What it needs |
|---|---|---|
| Flat fields | ICS-213, a relayed message, check-in and check-out | Today's seven types. |
| Status grids | Rows of Yes, No and Unknown; red, amber and green; hospital and shelter status | Today's `choice`, many times over. |
| Tables and logs | ICS-205 (radio channels), 211 (check-in list), 214 (activity log), 217A, and the communications logs groups keep (the widely used ICS 309 is not FEMA's) | Repeating rows. |
| Observations | Weather, storm and damage reports | Decimals with units, a date with its time, "check all that apply", a position. |
| Position and situation reports | The forms that Winlink's program maps | A position, and a receiver that maps and exports the reports. |

So the format needs a few more field types once. After that each new form is a JSON file in
`forms/`, not program code.

---

## 3. Six additions to the template format

RP-2 1.4.0 §8 is the normative text. In brief:

| Addition | A value is | For |
|---|---|---|
| Type `position` | `[lat, lon]` in ten-millionths of a degree (WGS 84), or `[lat, lon, within]` with how far off it may be, in metres | Every form that carries a position. |
| Type `datetime` | Minutes since 1970-01-01, UTC | When something was seen. ICS-213's "Reply date and time" is a `line` today for want of it. |
| Type `multi` | A bitmask over the field's `choices` | "Check all that apply". |
| `scale` and `unit` on `number` | The integer, read as having `scale` decimal places, shown with its unit | Rainfall 1.25 in, wind speed, temperature. |
| Type `rows` | An array of rows, each a map of column number to value; the field's `columns` are its columns | ICS-205, 211, 214, 217A, 309. |
| Type `heading` | No value | Sections in long forms (ICS-201, 209). |

**What it costs on the air.** A position is 11 bytes in CBOR at any precision from 10⁻⁴ to 10⁻⁷
degrees: each coordinate is a 5-byte integer **[T]**. The same position as text is 22 bytes. So
precision costs nothing; it is whatever the sender rounds to.

**How a position reads.** `33.78123, -84.38765 (EM73tt)`: signed decimal degrees, which any map
program or GPS takes as pasted, and the grid square amateurs use. Trailing zeros are dropped, so a
position rounded by its sender reads as rounded. A position that says how far off it may be adds
it: `33.5, -85.0 (EM73mm) ±108.1 km` for the centre of the grid square EM73.

**How a table reads.** The field's label alone, then the column labels and one line per row, the
cells joined by ` | `:

```
Communications log:
Time | From | To | Message
2026-11-24 14:05 UTC | KK4ODA | N0CALL | Shelter 3 open, 40 cots
```

**Older programs.** A station that cannot read a template already shows a form's values by number
(RP-2 §8.4). The change adds how a list or a map shows there: in CBOR diagnostic notation, as the
Rust library already writes it. Nothing an older program must do changes, so it is a MINOR change,
announced (RP-0 §8.4), and RadioPostKit is told.

**Left out on purpose.** Templates hold no scripts. Winlink's forms are HTML with JavaScript, which
is why anyone can write one and also why they break. RadioPost's are data: a total is typed by its
sender. No floating-point numbers: a decimal is an integer and a `scale`.

---

## 4. Forms that carry a position

### 4.1 Filling it in

W55 already decides where the station is: the GPS, else a saved place, else a grid square's
centre. That is the order Winlink's forms use too (§1). A position field starts with that position
and says where it came from: *GPS, 4 s ago*, or *Home, the centre of EM73*. A position read from a
grid square says how far off it may be (half the square's diagonal), so the reader is not misled.
The operator may type a position over it: decimal degrees, or a grid square. Degrees and minutes,
and USNG (which American emergency management uses), come later; they are only ways of typing and
showing it, never what goes on the air.

### 4.2 What goes

The position goes exactly as it is shown, by default (the author's decision, §9). W55 keeps the
precise position out of the client's log, so a form is the one place it is sent on purpose: the
composer shows the text that goes. The operator can send less by typing fewer decimals or a grid
square.

### 4.3 At the receiver (step 3)

The client keeps an index of every form received that has a position field: the form, its sender,
when, the position. A **Reports** view draws them on the client's map (`app/ui/map.js`), filtered
by form and by age. The operator picks a `choice` field of the form to colour the markers by, in
the order of its choices: the first green, the last red, *Unknown* and *Other* grey. (A template
key naming that field was the first idea; a published template never changes, so the view chooses
instead, the form's first required choice by default.) The reports export as CSV and as GeoJSON, which QGIS, ArcGIS and
CalTopo open. The client's land map is Natural Earth's 1:110m: right for a region, not for a
street. A street-level view would need map tiles from the Internet, as an option.

### 4.4 In email (step 3)

A post office writes a form's text into the email (RP-6 §3.1). Under `Maps:` after the form's
lines it adds, for each position, an OpenStreetMap link, which any mail program makes clickable,
and a `geo:` URI (RFC 5870) for map applications (the author's choice, §9). It costs no air time.
OpenStreetMap learns a position only when the reader clicks.

### 4.5 Position reports

W33 becomes one small template: position, course, speed, status and a comment, sent as mail to a
net's or an agency's address. A broadcast of it (RP-8's format) waits until a net shows that the
air time matters.

---

## 5. Where the templates come from

| Source | Forms | Terms | Plan |
|---|---|---|---|
| FEMA | The NIMS ICS Forms Booklet (FEMA 502-2): ICS 201 to 225, and ICS 213 RR as its Appendix B example | A United States government publication, in the public domain | ICS 202, 204, 205, 205A, 206, 208, 210, 211, 213, 213 RR and 214 published, each with its paper form (step 2). |
| The National Weather Service | What its offices ask SKYWARN spotters to report **[P]** (the offices' spotter guides: the events agree, the sizes and speeds differ) | Guidance, not a form | RadioPost's own Storm Report asks for those events, published (step 2). |
| California EMSA | HICS, the hospital forms | To be checked before publishing | Check the terms. |
| The ARRL | The radiogram | Permission needed | The author asks (§9). The template waits for the answer. |
| The American Red Cross | Its forms | Permission needed | The author asks (§9). The templates wait for the answer. |
| SHARES, MARS | Their programmes' forms | Their programmes' own | Not planned: only with their say-so. |
| No public original | Check-in and check-out, a communications log, a situation report, a damage report, a position report | RadioPost's own, under CC BY 4.0 | Published (step 2). The communications log does the job of the ICS 309, which is not FEMA's and whose terms are not known. |

Each published template follows `forms/README.md`: written once, never changed, a new one for any
change. A paper form gets a print layout (W53).

---

## 6. Templates by fingerprint (step 4)

A template is named by a hash of itself, so any copy of it proves itself. The author decided (§9)
that a post office serves templates by their fingerprint. That makes a group's own forms usable
without a new release of the programs, and closes a gap: a post office that does not hold a group's
template writes its email as `Field 7: …`, which an agency cannot use.

* **The post office serves them.** `GET /forms/{fingerprint}` answers the template's CBOR, or 404.
  No login: nothing on the air is secret, and the fingerprint is heard with every form.
* **Clients hand them in.** A client sending a form whose template is not published gives it to
  its post office (`PUT /forms/{fingerprint}`, logged in). The post office checks that it reads as
  a template and hashes to its name.
* **Between post offices.** A post office missing the template of a form from another post office's
  user asks that post office.
* **Clients fetch them.** A client that receives a form it has no template for asks its post office
  when it is online, and keeps what it gets under its first home.
* **By radio.** A template sent as a file of its own type in a message's `files`, kept by
  fingerprint once it checks.
* **By hand.** The client imports and exports template files, as RP-2 §8.1 already allows.

This needs RP-5 (MINOR) and a file type in RP-2 §8.3. A form designer in the client comes after
it: Winlink's library grew because anyone could write a form, and RadioPost's can grow the same
way, as data.

---

## 7. What is left out

* Scripts and computed fields in templates (§3).
* Print layouts from anywhere but the programs: a layout is HTML, so only the programs' own are
  shown (W53).
* Reading or writing Winlink's forms or their XML (W32, the author's decision).

---

## 8. The order of work

| Step | What | Status |
|---|---|---|
| 1 | RP-2 1.4.0: the six additions, their test vectors, the Rust library, the composer and the reader; a position field filled from W55. | Built |
| 2 | Templates: FEMA's ICS forms, a storm report, check-in and check-out, a situation report, a damage report and a position report, with print layouts for the paper forms. | Built |
| 3 | The Reports view: map, CSV and GeoJSON; map links in email (RP-6). | Built |
| 4 | Templates by fingerprint (§6): RP-5, RP-2 §8.3, the post office, the client. | Planned |
| 5 | Position reports as broadcasts, only if a net asks. | Not planned |

**Step 1 in detail.** RP-2 §8.1 to §8.4 gain the types, the keys `scale`, `unit` and `columns`,
the rules a template must keep, and each value's checks and text. `tools/vectors/rpref/forms.py`
and `make.py` make the vectors from a test template that uses every type, with every verdict
written out. `radiopost::forms` reads, checks and shows them, replayed against the vectors. The
client's composer has an input for each type; a position field starts with W55's position;
`compose --field` reads each type. No template is published in step 1: the test template lives in
`vectors/`, so an operator sees nothing new until step 2 publishes forms that use the types.

**What step 1 built.** RP-2 1.4.0 and `forms.json`'s `every_type`: a test template with every type,
two messages, 28 values written, 28 sets of values checked and 19 templates judged, every verdict
written out by hand in `make.py` and each check shown to fail on a planted fault in the reference
and in the Rust library. `compose::value_from_text` reads each type for the command line and the
panel; `compose::position_from_text` reads decimal degrees, a grid square (its centre, with half
its diagonal), and the text a form shows. The composer has an input for each type: a position
starts with W55's position, says where it came from and shows exactly what goes
(`/api/form-position`); a table adds and removes rows; Edit puts every type back. The reader shows a
heading across the form and a table as a table. ICS-213's composer, reader and paper form render
exactly as before.

**What step 2 built.** Eighteen published templates, in the order the programs hold them:

* FEMA's, from the NIMS ICS Forms Booklet (FEMA 502-2) read on 2026-11-24 **[P]**: ICS-213 (as
  before), 202, 204, 205, 205A, 206, 208, 210, 211, 213RR and 214. Each block is a field, in the
  form's order and words. A table is a `rows` field; a block of several dates and times (an
  operational period) is two `datetime` fields; the date and time the form was prepared are the
  message's own (bound), as ICS-213's are; a name with its signature is the name alone, the
  signature left to paper. Where a column holds two things, it became two columns: ICS-206's
  *Address, Latitude & Longitude if Helipad* is an address and a `position`. Each prints as
  FEMA's paper form (`forms/ics-*.print.html`), its tables padded with empty lines as on paper.
* RadioPost's own: Check-In, Check-Out, Communications Log, Damage Report (FEMA's damage levels:
  affected, minor, major, destroyed), Position Report, Situation Report and Storm Report (the
  events the National Weather Service's offices ask spotters to report, with the storm's movement
  and whether a wind speed was measured or estimated, as they ask). They print as they show.

Left for later: FEMA's ICS 201 (its sketch map is a picture), 203, 207 (an organization chart),
209 (over fifty blocks), 215 and 215A (planning worksheets), 218, 219 (cards), 220, 221 and 225;
HICS until its terms are checked; the ARRL's and the Red Cross's until they answer.

Dates and times are UTC on every form, as on ICS-213, though FEMA's instructions say local time is
typical: one clock for every station on a net, and the one the message's own date is in.

**What step 3 built.** RP-6 1.3.0 §3.1: a form's positions, a field's or a table's cells, each
get a line under `Maps:` in the email, the OpenStreetMap link and the `geo:` URI, the map opening
closer the surer the position (`email-out.json`'s `shelter3-storm` and `shelter3-206`, and
`map_zooms`). The client's **Reports** (`app/client/src/reports.rs`, `/api/reports`): every form in
the Inbox and Sent that carries a position, read afresh each time (a message deleted is a report
gone), on the map of the Gateways view (zooming to a town, with subsquares) and in a list, by form
and age, coloured by a choice; **Open the message**; **OpenStreetMap**, which the desktop app
opens in the system's browser from a link the client makes from the numbers alone; **Save as
CSV** (RFC 4180, a line for each position, one form's fields as columns) and **Save as GeoJSON**
(RFC 7946, a point for each position), in the Downloads folder.

---

## 9. The author's decisions (2026-11-24)

1. **A post office serves templates by their fingerprint** (§6).
2. **The ARRL's and the Red Cross's forms wait for their permission**, which the author asks for
   (§5). Step 2 starts with the public-domain forms.
3. **A position goes exactly as it is, by default** (§4.2). The operator may send less.
4. **An emailed position gets both links**, OpenStreetMap's and a `geo:` URI (§4.4).

---

## Change log

| Version | Date | Change |
|---|---|---|
| 0.1.0 | 2026-11-24 | First version, accepted with the author's three answers (§9); step 1 built (§8). |
| 0.2.0 | 2026-11-24 | Step 2 built (§8): eleven of FEMA's ICS forms with their paper forms, and seven of RadioPost's own. §2, §5: the ICS 309 is not FEMA's; ICS 213 RR is FEMA's Appendix B example; the storm report follows the National Weather Service's spotter guidance. |
| 0.3.0 | 2026-11-24 | Step 3 built (§8): the Reports view and map links in email (RP-6 1.3.0), with the author's fourth answer (§9). §4.3: the view, not the template, chooses the field that colours the markers. |
