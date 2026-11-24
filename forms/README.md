# RadioPost forms

The templates RadioPost publishes for forms sent as messages (RP-2 §8). A form message carries
only its values, by field number; each station holds the template and makes the form's text from
it. A template is named by its fingerprint, the first 8 bytes of SHA-256 of its CBOR encoding, so
a changed template is another template.

| File | Form | From | On paper | Fingerprint |
|---|---|---|---|---|
| `ics-213.json` | ICS-213 General Message | FEMA's, public domain | `ics-213.print.html` | `25393c52580e7a3c` |
| `ics-202.json` | ICS-202 Incident Objectives | FEMA's, public domain | `ics-202.print.html` | `707095647c62045a` |
| `ics-204.json` | ICS-204 Assignment List | FEMA's, public domain | `ics-204.print.html` | `fb40811f7dab39d1` |
| `ics-205.json` | ICS-205 Incident Radio Communications Plan | FEMA's, public domain | `ics-205.print.html` | `a835c64bd05ede4e` |
| `ics-205a.json` | ICS-205A Communications List | FEMA's, public domain | `ics-205a.print.html` | `de534486d9943a00` |
| `ics-206.json` | ICS-206 Medical Plan | FEMA's, public domain | `ics-206.print.html` | `5cf57eeb404d7beb` |
| `ics-208.json` | ICS-208 Safety Message/Plan | FEMA's, public domain | `ics-208.print.html` | `c9493d08fd4834dd` |
| `ics-210.json` | ICS-210 Resource Status Change | FEMA's, public domain | `ics-210.print.html` | `a8c834c7dd268016` |
| `ics-211.json` | ICS-211 Incident Check-In List | FEMA's, public domain | `ics-211.print.html` | `534c4144e5b784c9` |
| `ics-213rr.json` | ICS-213RR Resource Request Message | FEMA's, public domain | `ics-213rr.print.html` | `48d358185d4ef34b` |
| `ics-214.json` | ICS-214 Activity Log | FEMA's, public domain | `ics-214.print.html` | `15c064dab95d5c36` |
| `check-in.json` | Check-in | RadioPost's own | prints as it shows | `503bf9b35bd047f1` |
| `check-out.json` | Check-out | RadioPost's own | prints as it shows | `6ed311f6d0076eb8` |
| `comms-log.json` | Communications Log | RadioPost's own | prints as it shows | `c071db80be8d9501` |
| `damage-report.json` | Damage Report | RadioPost's own | prints as it shows | `9926454716eae307` |
| `position-report.json` | Position Report | RadioPost's own | prints as it shows | `5cf82a001bbfabf1` |
| `situation-report.json` | Situation Report | RadioPost's own | prints as it shows | `d2ae69eef3afe982` |
| `storm-report.json` | Storm Report | RadioPost's own | prints as it shows | `58584708ebf060c1` |

FEMA's forms follow the NIMS ICS Forms Booklet (FEMA 502-2), a United States government
publication in the public domain; ICS-213RR is its Appendix B example. Their dates and times are UTC,
and their signatures are left to the paper form. FEMA's ICS 201 (a sketch map), 203, 207 (an
organization chart), 209, 215, 215A, 218, 219 (cards), 220, 221 and 225 are not published yet; ICS 309
is not FEMA's, and RadioPost's own Communications Log does its job. RadioPost's own forms are
published under CC BY 4.0, as the rest of this directory is. [Forms for most jobs](../docs/design/06-forms.md)
says how they were chosen.

Each file is the template as JSON: `name`, an optional `note`, and `fields`, each with its number
`n`, its `label` and `type` (`line`, `text`, `date`, `time`, `number`, `choice`, `check`,
`position`, `datetime`, `multi`, `rows`, `heading`), and, where they apply, `required`, `choices`,
`bound` (`subject`, `date`, `time`), `max`, a number's `scale` and `unit`, and a `rows` field's
`columns` (fields of their own). RP-2 §8.1 says how it becomes the CBOR that is fingerprinted and
the rules a template keeps; `vectors/forms.json`'s `every_type` is a test template that uses them
all. [Forms for most jobs](../docs/design/06-forms.md) says which forms come next.

**Print layouts.** `ics-*.print.html` are how the client prints FEMA's forms: as FEMA's paper
forms, their blocks numbered as there. A layout is the programs' own and never goes on the air, so
it is a file of its own, not part of the template: it names the template it lays out by its
fingerprint (`data-template`), and has a place for each field but a heading (`data-field`, the
field's number), where the client puts the value as text. A table's place is a `tbody`, which gets
the rows and is padded with empty lines to its `data-rows`. A form with no layout prints as it
shows. The client holds the layouts listed in `radiopost::forms::LAYOUTS`, whose test checks that
each has a place for every field of its template.

To add a form, write its template here, add it to `PUBLISHED_FORMS` in `tools/vectors/make.py` so
that `vectors/forms.json` gives its fingerprint, and to `radiopost::forms::PUBLISHED` so the
programs hold it. Never change a published template: write a new one.

These templates and layouts are published under CC BY 4.0, as the specifications are.
