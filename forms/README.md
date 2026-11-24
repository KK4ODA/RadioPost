# RadioPost forms

The templates RadioPost publishes for forms sent as messages (RP-2 §8). A form message carries
only its values, by field number; each station holds the template and makes the form's text from
it. A template is named by its fingerprint, the first 8 bytes of SHA-256 of its CBOR encoding, so
a changed template is another template.

| File | Form | Fingerprint |
|---|---|---|
| `ics-213.json` | ICS-213 General Message, after FEMA's public-domain form | `vectors/forms.json` gives it |

Each file is the template as JSON: `name`, an optional `note`, and `fields`, each with its number
`n`, its `label` and `type` (`line`, `text`, `date`, `time`, `number`, `choice`, `check`,
`position`, `datetime`, `multi`, `rows`, `heading`), and, where they apply, `required`, `choices`,
`bound` (`subject`, `date`, `time`), `max`, a number's `scale` and `unit`, and a `rows` field's
`columns` (fields of their own). RP-2 §8.1 says how it becomes the CBOR that is fingerprinted and
the rules a template keeps; `vectors/forms.json`'s `every_type` is a test template that uses them
all. [Forms for most jobs](../docs/design/06-forms.md) says which forms come next.

**Print layouts.** `ics-213.print.html` is how the client prints an ICS-213: as FEMA's ICS 213,
the paper form. A layout is the programs' own and never goes on the air, so it is a file of its
own, not part of the template: it names the template it lays out by its fingerprint
(`data-template`), and has a place for each field (`data-field`, the field's number), where the
client puts the value as text. A form with no layout prints as it shows. The client holds the
layouts listed in `radiopost::forms::LAYOUTS`, whose test checks that each has a place for every
field of its template.

To add a form, write its template here, add it to `tools/vectors/make.py` so that `vectors/forms.json`
gives its fingerprint, and let the programs hold it. Never change a published template: write a new
one.

These templates and layouts are published under CC BY 4.0, as the specifications are.
